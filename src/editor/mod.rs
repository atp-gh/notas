//! The editor pane: GtkSourceView-based Markdown editor, a rendered
//! preview, and a find/replace bar.
//!
//! Layout is a horizontal split: source on the left, live preview on the
//! right. Preview rendering is async and debounced — keystrokes only
//! schedule work, the expensive `markdown::render_themed` step runs on a
//! background thread, and only the cheap bulk insert + tag pass touches the
//! UI thread.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use libadwaita as adw;
use sourceview5::prelude::*;

use crate::editor::preview::{PreviewTags, apply_rendered, apply_scheme};
use crate::ui::protocol::{AppMsg, EditorMode};

mod preview;

/// Debounce for live preview keystrokes: typing only reschedules this.
const LIVE_DEBOUNCE: Duration = Duration::from_millis(150);

/// Scroll position of `adj` as a fraction of its scrollable range.
///
/// Returns `0.0` when nothing is scrollable. Pure so scroll sync and
/// position memory stay testable without GTK.
fn scroll_frac(adj: &gtk::Adjustment) -> f64 {
    scroll_frac_of(adj.value(), adj.upper(), adj.page_size())
}

/// Scroll position as a fraction of `value` within its scrollable range.
fn scroll_frac_of(value: f64, upper: f64, page: f64) -> f64 {
    let max = upper - page;
    if max > 0.0 {
        (value / max).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Absolute adjustment value for `frac` of the scrollable range.
fn scroll_value_of(frac: f64, upper: f64, page: f64) -> f64 {
    (frac.clamp(0.0, 1.0) * (upper - page).max(0.0)).max(0.0)
}

/// Fraction a newly shown side should restore.
///
/// A side that was hidden holds a stale fraction (recorded long ago, possibly
/// for different content); a side that was visible was just snapshotted
/// fresh. So a newly shown side adopts the visible side's fraction, falling
/// back to its own when the other side never recorded one. Pure so the
/// inheritance rule stays testable without GTK.
fn inherit_frac(
    newly_shown: bool,
    from_visible: bool,
    from: Option<f64>,
    own: Option<f64>,
) -> Option<f64> {
    if newly_shown && from_visible {
        from.or(own)
    } else {
        own
    }
}

/// Install a display-wide CSS override so the source view's base
/// background/foreground track the GTK theme (`view_bg_color` /
/// `view_fg_color`). Installed once; harmless if called again.
fn ensure_editor_theme_css() {
    use std::sync::OnceLock;
    static DONE: OnceLock<()> = OnceLock::new();
    if DONE.get().is_some() {
        return;
    }
    let css = gtk::CssProvider::new();
    css.load_from_string(
        "sourceview text, sourceview viewport, textview text, textview viewport { \
           background-color: var(--view-bg-color); \
           color: var(--view-fg-color); \
         }",
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        // Leak the provider so it outlives this call; the display holds a
        // ref, and we only need it installed once per process.
        std::mem::forget(css);
        DONE.set(()).ok();
    }
}

pub struct Editor {
    pub source_view: sourceview5::View,
    pub source_buffer: sourceview5::Buffer,
    /// Left = source scroll, right = preview scroll. Mode buttons show or
    /// hide each child instead of swapping stack pages, so both stay
    /// allocated and live.
    pub split: gtk::Paned,
    source_scroll: gtk::ScrolledWindow,
    preview_scroll: gtk::ScrolledWindow,
    pub search_bar: gtk::SearchBar,
    pub search_entry: gtk::SearchEntry,
    pub search_context: sourceview5::SearchContext,
    search_settings: sourceview5::SearchSettings,
    live_rev: Rc<Cell<u64>>,
    live_tx: std::sync::mpsc::Sender<(u64, crate::markdown::RenderedMarkdown)>,
    live_debounce: Rc<RefCell<Option<glib::SourceId>>>,
    /// Re-entrancy guard for scroll sync: programmatic `set_value` calls fire
    /// `value-changed` synchronously, so the follower must not drive back.
    sync_guard: Rc<Cell<bool>>,
    /// Remembered scroll fractions per side (`None` = never visited).
    source_frac: Rc<Cell<Option<f64>>>,
    preview_frac: Rc<Cell<Option<f64>>>,
    /// Fraction to re-apply when the preview layout settles (`None` = none
    /// pending). `set_text` resets to the top and the new `upper` only lands
    /// asynchronously — longer with diagram anchors — so a synchronous
    /// restore right after render uses stale bounds and drifts once layout
    /// completes. The adjustment's `changed` handler below performs the
    /// restore against final bounds instead.
    preview_pending: Rc<Cell<Option<f64>>>,
}

/// Render `source_buffer` on a background thread, delivering the result to
/// the preview buffer on the UI thread. Stale revisions are dropped.
fn spawn_preview_render(
    rev_counter: &Rc<Cell<u64>>,
    tx: &std::sync::mpsc::Sender<(u64, crate::markdown::RenderedMarkdown)>,
    source_buffer: &sourceview5::Buffer,
    dark: bool,
) {
    let rev = rev_counter.get().wrapping_add(1);
    rev_counter.set(rev);
    let md: String = source_buffer
        .text(&source_buffer.start_iter(), &source_buffer.end_iter(), true)
        .into();
    let tx = tx.clone();
    std::thread::spawn(move || {
        let rendered = crate::markdown::render_themed(&md, dark);
        let _ = tx.send((rev, rendered));
        glib::MainContext::default().wakeup();
    });
}

impl Editor {
    /// Queue an async preview render now (no debounce). No-op in
    /// [`EditorMode::Source`].
    pub fn request_preview_immediate(&self) {
        if self.editor_mode() == EditorMode::Source {
            return;
        }
        spawn_preview_render(
            &self.live_rev,
            &self.live_tx,
            &self.source_buffer,
            adw::StyleManager::default().is_dark(),
        );
    }

    /// Switch the split layout. Modes carrying a preview trigger an
    /// immediate async render; collapsing to source-only cancels pending
    /// live work. Visible sides keep their scroll fractions across switches;
    /// a side visited for the first time inherits the other side's fraction.
    pub fn set_editor_mode(&self, mode: EditorMode) {
        let source_was = self.source_scroll.is_visible();
        let preview_was = self.preview_scroll.is_visible();
        self.remember_visible_scroll();
        match mode {
            EditorMode::Source => {
                self.source_scroll.set_visible(true);
                self.preview_scroll.set_visible(false);
                if let Some(id) = self.live_debounce.borrow_mut().take() {
                    id.remove();
                }
            }
            EditorMode::Split => {
                self.source_scroll.set_visible(true);
                self.preview_scroll.set_visible(true);
                self.request_preview_immediate();
            }
            EditorMode::Preview => {
                self.source_scroll.set_visible(false);
                self.preview_scroll.set_visible(true);
                self.request_preview_immediate();
            }
        }
        self.inherit_newly_shown(source_was, preview_was);
        self.restore_visible_scroll();
    }

    /// Snapshot the scroll fractions of the currently visible sides.
    fn remember_visible_scroll(&self) {
        if self.source_scroll.is_visible() {
            self.source_frac
                .set(Some(scroll_frac(&self.source_scroll.vadjustment())));
        }
        if self.preview_scroll.is_visible() {
            self.preview_frac
                .set(Some(scroll_frac(&self.preview_scroll.vadjustment())));
        }
    }

    /// Inherit fractions for sides newly shown by a mode switch.
    ///
    /// A side that was hidden holds a stale fraction; the side that was
    /// visible was just snapshotted fresh. Without this, Source→Preview
    /// restores the preview's stale Split-era `0.0` instead of carrying the
    /// reading position across.
    fn inherit_newly_shown(&self, source_was: bool, preview_was: bool) {
        if self.source_scroll.is_visible() {
            self.source_frac.set(inherit_frac(
                !source_was,
                preview_was,
                self.preview_frac.get(),
                self.source_frac.get(),
            ));
        }
        if self.preview_scroll.is_visible() {
            self.preview_frac.set(inherit_frac(
                !preview_was,
                source_was,
                self.source_frac.get(),
                self.preview_frac.get(),
            ));
        }
    }

    /// Forget remembered scroll fractions, e.g. when a different note loads.
    /// Call before the new content lands so neither side restores the old
    /// note's position (the source reset happens on its own via the buffer
    /// change; the preview re-render would otherwise jump to stale state).
    pub fn reset_scroll_memory(&self) {
        self.source_frac.set(None);
        self.preview_frac.set(None);
        self.preview_pending.set(None);
    }

    /// Re-apply remembered fractions to the currently visible sides.
    /// The preview re-render (`set_text`) resets its adjustment to the top,
    /// so the async drain re-applies this again when fresh content lands.
    fn restore_visible_scroll(&self) {
        if self.source_scroll.is_visible() {
            Self::restore_scroll(
                &self.source_scroll,
                self.source_frac.get(),
                &self.sync_guard,
            );
        }
        if self.preview_scroll.is_visible() {
            Self::restore_scroll(
                &self.preview_scroll,
                self.preview_frac.get(),
                &self.sync_guard,
            );
        }
    }

    /// Move `scroll` to `frac` without triggering scroll sync or position
    /// memory writes. No-op when `frac` was never recorded.
    fn restore_scroll(scroll: &gtk::ScrolledWindow, frac: Option<f64>, guard: &Rc<Cell<bool>>) {
        let Some(frac) = frac else { return };
        let adj = scroll.vadjustment();
        guard.set(true);
        adj.set_value(scroll_value_of(frac, adj.upper(), adj.page_size()));
        guard.set(false);
    }

    /// Current layout derived from child visibility (the single source of
    /// truth, so button state can never drift from the widgets).
    pub fn editor_mode(&self) -> EditorMode {
        match (
            self.source_scroll.is_visible(),
            self.preview_scroll.is_visible(),
        ) {
            (true, false) => EditorMode::Source,
            (false, true) => EditorMode::Preview,
            _ => EditorMode::Split,
        }
    }

    pub fn set_search_text(&self, text: Option<&str>) {
        self.search_settings.set_search_text(text);
    }

    /// Move to the next match (from the current selection if any).
    pub fn find_next(&self) {
        let buffer = &self.source_buffer;
        let iter = if buffer.has_selection() {
            buffer.iter_at_mark(&buffer.selection_bound())
        } else {
            buffer.start_iter()
        };
        if let Some((start, end, _wrapped)) = self.search_context.forward(&iter) {
            buffer.select_range(&start, &end);
            let mut start = start;
            self.source_view
                .scroll_to_iter(&mut start, 0.0, false, 0.0, 0.0);
        }
    }

    pub fn find_prev(&self) {
        let buffer = &self.source_buffer;
        let iter = if buffer.has_selection() {
            buffer.iter_at_mark(&buffer.selection_bound())
        } else {
            buffer.start_iter()
        };
        if let Some((start, end, _wrapped)) = self.search_context.backward(&iter) {
            buffer.select_range(&start, &end);
            let mut start = start;
            self.source_view
                .scroll_to_iter(&mut start, 0.0, false, 0.0, 0.0);
        }
    }

    /// Replace every match of the current search text, returning the count.
    pub fn replace_all(&self, replacement: &str) -> usize {
        let buffer = &self.source_buffer;
        let mut iter = buffer.start_iter();
        let mut count = 0;
        while let Some((start, end, _wrapped)) = self.search_context.forward(&iter) {
            let mut s = start;
            let mut e = end;
            if self
                .search_context
                .replace(&mut s, &mut e, replacement)
                .is_ok()
            {
                count += 1;
            }
            iter = e;
        }
        count
    }
}

/// Build the whole editor pane. `emit` forwards UI events to the app
/// component; `suppress` gates the "changed" signal so that programmatic
/// loads don't mark the note dirty.
pub fn build_editor<E>(emit: E, suppress: Rc<Cell<bool>>) -> Editor
where
    E: Fn(AppMsg) + 'static + Clone,
{
    let source_buffer = sourceview5::Buffer::new(None);
    source_buffer.set_highlight_syntax(true);
    source_buffer.set_max_undo_levels(5000);

    // Markdown highlighting ships with GtkSourceView 5.
    let manager = sourceview5::LanguageManager::new();
    if let Some(lang) = manager.language("markdown") {
        source_buffer.set_language(Some(&lang));
    }

    {
        let emit = emit.clone();
        let suppress_flag = suppress.clone();
        source_buffer.connect_changed(move |_| {
            if !suppress_flag.get() {
                emit(AppMsg::ContentChanged);
            }
        });
    }

    let source_view = sourceview5::View::new();
    source_view.set_buffer(Some(&source_buffer));
    source_view.set_monospace(true);
    source_view.set_show_line_numbers(false);
    source_view.set_show_right_margin(false);
    source_view.set_wrap_mode(gtk::WrapMode::WordChar);
    // GtkSourceView style schemes (Adwaita/Adwaita-dark below) carry their
    // own text background, ignoring the GTK theme. Force the base
    // background/foreground back to the theme's view colors so any system
    // theme is honored; per-token syntax colors from the scheme still apply
    // on top via tags.
    ensure_editor_theme_css();

    let preview_buffer = gtk::TextBuffer::new(None);
    let preview_tags = Rc::new(PreviewTags::new(&preview_buffer));

    let preview_view = gtk::TextView::new();
    preview_view.set_buffer(Some(&preview_buffer));
    preview_view.set_editable(false);
    preview_view.set_cursor_visible(false);
    preview_view.set_wrap_mode(gtk::WrapMode::WordChar);
    preview_view.set_left_margin(8);
    preview_view.set_right_margin(8);
    preview_view.set_top_margin(8);
    preview_view.set_bottom_margin(8);

    // Async live pipeline: background threads `send` rendered documents
    // over std mpsc; the drain is installed after `split` exists so it can
    // also restore the remembered preview scroll (see below).
    let live_rev = Rc::new(Cell::new(0u64));
    let live_debounce: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let (live_tx, live_rx) = std::sync::mpsc::channel::<(u64, crate::markdown::RenderedMarkdown)>();
    // Scroll sync + position memory. Fractions, not pixels: the two sides
    // have different content heights, so only percentages transfer.
    let sync_guard = Rc::new(Cell::new(false));
    let source_frac: Rc<Cell<Option<f64>>> = Rc::new(Cell::new(None));
    let preview_frac: Rc<Cell<Option<f64>>> = Rc::new(Cell::new(None));
    let preview_pending: Rc<Cell<Option<f64>>> = Rc::new(Cell::new(None));

    // Keep the preview colors and the syntax highlighting scheme in sync
    // with the libadwaita theme. GtkSourceView defaults to a light scheme
    // and does not switch it automatically, so apply it ourselves.
    // Preview muted colors are sampled from the preview widget's theme
    // (see PreviewTags::apply_theme) so they track any system theme.
    let scheme_manager = sourceview5::StyleSchemeManager::new();
    {
        let style_manager = adw::StyleManager::default();
        let dark = style_manager.is_dark();
        let accent = style_manager.accent_color_rgba();
        preview_tags.apply_theme(&preview_view, dark, &accent);
        apply_scheme(&scheme_manager, &source_buffer, dark);

        let tags = preview_tags.clone();
        let sm = style_manager.clone();
        let buf = source_buffer.clone();
        let rev = live_rev.clone();
        let tx = live_tx.clone();
        let src = source_buffer.clone();
        let pv = preview_view.clone();
        sm.clone().connect_dark_notify(move |_| {
            let dark = sm.is_dark();
            tags.apply_theme(&pv, dark, &sm.accent_color_rgba());
            apply_scheme(&scheme_manager, &buf, dark);
            // Syntax token colors are baked at render time, so queue an
            // async re-render to pick the matching light/dark tmTheme.
            spawn_preview_render(&rev, &tx, &src, dark);
        });

        let tags = preview_tags.clone();
        let sm = style_manager;
        let pv = preview_view.clone();
        sm.clone().connect_accent_color_rgba_notify(move |_| {
            tags.apply_theme(&pv, sm.is_dark(), &sm.accent_color_rgba());
        });
    }

    // Clickable links: hover shows a pointer cursor, clicking opens the
    // URL in the system browser.
    {
        let tags = preview_tags.clone();
        let view = preview_view.clone();
        let gesture = gtk::GestureClick::new();
        gesture.connect_pressed(move |_g, _n, x, y| {
            if let Some(iter) = view.iter_at_location(x as i32, y as i32)
                && let Some(url) = tags.link_url_at(iter.offset())
            {
                let _ = gtk::gio::AppInfo::launch_default_for_uri(
                    &url,
                    None::<&gtk::gio::AppLaunchContext>,
                );
            }
        });
        preview_view.add_controller(gesture);

        let tags = preview_tags.clone();
        let view = preview_view.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(move |_m, x, y| {
            let over_link = view
                .iter_at_location(x as i32, y as i32)
                .is_some_and(|iter| tags.link_url_at(iter.offset()).is_some());
            let cursor = if over_link {
                gtk::gdk::Cursor::from_name("pointer", None)
            } else {
                None
            };
            view.set_cursor(cursor.as_ref());
        });
        preview_view.add_controller(motion);
    }

    // A TextView only gets a real viewport (and scrollbars) inside a
    // ScrolledWindow; without one the views grew to their full content
    // height and could not be scrolled.
    let source_scroll = gtk::ScrolledWindow::new();
    source_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    source_scroll.set_child(Some(&source_view));
    source_scroll.set_vexpand(true);
    source_scroll.set_hexpand(true);

    let preview_scroll = gtk::ScrolledWindow::new();
    preview_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    preview_scroll.set_child(Some(&preview_view));
    preview_scroll.set_vexpand(true);
    preview_scroll.set_hexpand(true);

    // Live split: source left, preview right. The Preview toggle shows or
    // hides the right child; both stay allocated so typing never reparents.
    let split = gtk::Paned::new(gtk::Orientation::Horizontal);
    split.set_start_child(Some(&source_scroll));
    split.set_end_child(Some(&preview_scroll));
    split.set_position(500);
    split.set_shrink_start_child(false);
    split.set_shrink_end_child(false);
    split.set_resize_start_child(true);
    split.set_resize_end_child(true);
    split.set_wide_handle(true);

    // Bidirectional scroll sync, active only while both sides are visible
    // (split mode). Each side records its own fraction on every move so mode
    // switches can restore it. Programmatic moves hold the guard: `set_value`
    // fires `value-changed` synchronously, and the follower must neither
    // record nor drive back.
    {
        let guard = sync_guard.clone();
        let own = source_frac.clone();
        let other_adj = preview_scroll.vadjustment();
        let own_scroll = source_scroll.clone();
        let other_scroll = preview_scroll.clone();
        let own_adj = source_scroll.vadjustment();
        own_adj.connect_value_changed(move |adj| {
            if guard.get() {
                return;
            }
            let frac = scroll_frac(adj);
            own.set(Some(frac));
            if !own_scroll.is_visible() || !other_scroll.is_visible() {
                return;
            }
            guard.set(true);
            other_adj.set_value(scroll_value_of(
                frac,
                other_adj.upper(),
                other_adj.page_size(),
            ));
            guard.set(false);
        });
        let guard = sync_guard.clone();
        let own = preview_frac.clone();
        let pending = preview_pending.clone();
        let other_adj = source_scroll.vadjustment();
        let own_scroll = preview_scroll.clone();
        let other_scroll = source_scroll.clone();
        let own_adj = preview_scroll.vadjustment();
        own_adj.connect_value_changed(move |adj| {
            if guard.get() {
                return;
            }
            // A real user move supersedes any layout restore queued by the
            // last render; without this a later validation pass would yank
            // the view back.
            pending.set(None);
            let frac = scroll_frac(adj);
            own.set(Some(frac));
            if !own_scroll.is_visible() || !other_scroll.is_visible() {
                return;
            }
            guard.set(true);
            other_adj.set_value(scroll_value_of(
                frac,
                other_adj.upper(),
                other_adj.page_size(),
            ));
            guard.set(false);
        });
        // Layout-completion restore: `set_text` queues a relayout and the new
        // `upper` lands asynchronously (`changed` fires per validation pass),
        // so re-apply the pending fraction against final bounds. `pending` is
        // only `Some` right after a render with no user scroll since, hence a
        // later window resize restoring the same fraction is a no-op in
        // effect. `set_value` emits `value-changed`, never `changed`, so this
        // cannot recurse.
        {
            let guard = sync_guard.clone();
            let pending = preview_pending.clone();
            preview_scroll.vadjustment().connect_changed(move |adj| {
                if guard.get() {
                    return;
                }
                if let Some(frac) = pending.get() {
                    guard.set(true);
                    adj.set_value(scroll_value_of(frac, adj.upper(), adj.page_size()));
                    guard.set(false);
                }
            });
        }
    }

    // Drain for the async live pipeline: applies the latest revision only.
    // Workers `wakeup` the main context so this runs promptly even when the
    // loop is idle. `apply_rendered` resets the preview to the top via
    // `set_text`, so the remembered fraction is re-applied under the guard
    // (which also keeps the reset itself from overwriting the memory); the
    // same fraction is queued as pending for the layout-completion restore
    // above, which corrects the drift once final bounds land.
    {
        let rev = live_rev.clone();
        let view = preview_view;
        let buf = preview_buffer;
        let tags = preview_tags;
        let guard = sync_guard.clone();
        let frac = preview_frac.clone();
        let pending = preview_pending.clone();
        let scroll = preview_scroll.clone();
        glib::idle_add_local(move || {
            let mut latest = None;
            while let Ok(item) = live_rx.try_recv() {
                latest = Some(item);
            }
            if let Some((msg_rev, rendered)) = latest
                && rev.get() == msg_rev
            {
                guard.set(true);
                apply_rendered(&view, &buf, &tags, &rendered);
                guard.set(false);
                pending.set(frac.get());
                Editor::restore_scroll(&scroll, frac.get(), &guard);
            }
            glib::ControlFlow::Continue
        });
    }

    // Debounced live preview, wired after `preview_scroll` exists so the
    // fire-time visibility check is truthful. Programmatic loads set
    // `suppress` and therefore skip this; they call
    // `request_preview_immediate` explicitly instead.
    {
        let emit = emit.clone();
        let suppress_live = suppress;
        let rev = live_rev.clone();
        let tx = live_tx.clone();
        let debounce = live_debounce.clone();
        let preview_scroll_for_live = preview_scroll.clone();
        source_buffer.connect_changed(move |buffer| {
            if !suppress_live.get() {
                emit(AppMsg::ContentChanged);
                if let Some(id) = debounce.borrow_mut().take() {
                    id.remove();
                }
                let rev2 = rev.clone();
                let tx2 = tx.clone();
                let debounce2 = debounce.clone();
                let buffer2 = buffer.clone();
                let visible = preview_scroll_for_live.clone();
                let id = glib::timeout_add_local_once(LIVE_DEBOUNCE, move || {
                    *debounce2.borrow_mut() = None;
                    if !visible.is_visible() {
                        return;
                    }
                    spawn_preview_render(
                        &rev2,
                        &tx2,
                        &buffer2,
                        adw::StyleManager::default().is_dark(),
                    );
                });
                *debounce.borrow_mut() = Some(id);
            }
        });
    }

    // --- find / replace -----------------------------------------------------
    let search_settings = sourceview5::SearchSettings::new();
    search_settings.set_wrap_around(true);
    let search_context = sourceview5::SearchContext::new(&source_buffer, Some(&search_settings));
    search_context.set_highlight(true);

    let search_entry = gtk::SearchEntry::new();
    search_entry.set_hexpand(true);
    search_entry.set_placeholder_text(Some(crate::tr!("Find…")));
    search_entry.set_width_chars(24);

    let replace_entry = gtk::Entry::new();
    replace_entry.set_placeholder_text(Some(crate::tr!("Replace with…")));
    replace_entry.set_width_chars(18);

    let replace_all_btn = gtk::Button::with_label(crate::tr!("Replace all"));
    replace_all_btn.set_tooltip_text(Some(crate::tr!("Replace every match")));

    let find_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    find_row.append(&search_entry);
    find_row.append(&replace_entry);
    find_row.append(&replace_all_btn);

    let search_bar = gtk::SearchBar::new();
    search_bar.set_child(Some(&find_row));
    search_bar.connect_entry(&search_entry);
    search_bar.set_key_capture_widget(Some(&source_view));

    {
        let emit = emit.clone();
        search_entry.connect_search_changed(move |entry| {
            let text = entry.text().to_string();
            emit(AppMsg::FindChanged(text));
        });
    }
    {
        let next = emit.clone();
        search_entry.connect_next_match(move |_| next(AppMsg::FindNext));
        let prev = emit.clone();
        search_entry.connect_previous_match(move |_| prev(AppMsg::FindPrev));
    }
    {
        replace_all_btn.connect_clicked(move |_| {
            let text = replace_entry.text().to_string();
            emit(AppMsg::ReplaceAll(text));
        });
    }

    Editor {
        source_view,
        source_buffer,
        split,
        source_scroll,
        preview_scroll,
        search_bar,
        search_entry,
        search_context,
        search_settings,
        live_rev,
        live_tx,
        live_debounce,
        sync_guard,
        source_frac,
        preview_frac,
        preview_pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_frac_of_mid_range() {
        assert_eq!(scroll_frac_of(50.0, 110.0, 10.0), 0.5);
    }

    #[test]
    fn scroll_frac_of_clamps_to_unit_interval() {
        assert_eq!(scroll_frac_of(-5.0, 110.0, 10.0), 0.0);
        assert_eq!(scroll_frac_of(999.0, 110.0, 10.0), 1.0);
    }

    #[test]
    fn scroll_frac_of_empty_range_is_zero() {
        assert_eq!(scroll_frac_of(0.0, 100.0, 100.0), 0.0);
    }

    #[test]
    fn scroll_value_of_mid_fraction() {
        assert_eq!(scroll_value_of(0.5, 110.0, 10.0), 50.0);
    }

    #[test]
    fn scroll_value_of_clamps_fraction() {
        assert_eq!(scroll_value_of(2.0, 110.0, 10.0), 100.0);
    }

    #[test]
    fn scroll_frac_and_value_roundtrip() {
        let frac = scroll_frac_of(30.0, 130.0, 30.0);
        assert!((scroll_value_of(frac, 130.0, 30.0) - 30.0).abs() < 1e-9);
    }

    #[test]
    fn inherit_adopts_fresh_side_when_newly_shown() {
        assert_eq!(inherit_frac(true, true, Some(0.6), Some(0.0)), Some(0.6));
    }

    #[test]
    fn inherit_keeps_own_when_already_visible() {
        assert_eq!(inherit_frac(false, true, Some(0.6), Some(0.2)), Some(0.2));
    }

    #[test]
    fn inherit_keeps_own_when_other_side_hidden() {
        assert_eq!(inherit_frac(true, false, Some(0.6), Some(0.0)), Some(0.0));
    }

    #[test]
    fn inherit_keeps_own_when_other_never_recorded() {
        assert_eq!(inherit_frac(true, true, None, Some(0.0)), Some(0.0));
    }

    /// Manual scrollability probe (run with `--ignored --nocapture`): prints
    /// the source view's vertical adjustment range inside a constrained
    /// window.
    #[test]
    #[ignore = "manual probe: needs a display and visual inspection"]
    fn source_view_scroll_probe() {
        gtk::init().expect("gtk init");
        adw::init().expect("adw init");
        let editor = build_editor(|_| {}, Rc::new(Cell::new(false)));
        let long = (0..300)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        editor.source_buffer.set_text(&long);

        let window = gtk::Window::new();
        window.set_default_size(600, 300);
        let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pane.append(&editor.split);
        pane.set_vexpand(true);
        window.set_child(Some(&pane));
        window.present();
        let ctx = glib::MainContext::default();
        for _ in 0..50 {
            ctx.iteration(false);
        }

        let adj = editor
            .source_view
            .vadjustment()
            .expect("source vadjustment");
        let range = adj.upper() - adj.page_size();
        println!(
            "source view: upper={:.1} page={:.1} value={:.1} range={:.1}",
            adj.upper(),
            adj.page_size(),
            adj.value(),
            range
        );
        assert!(adj.page_size() > 100.0, "viewport must have real height");
        assert!(range > 0.0, "long content must be scrollable");
        adj.set_value(range);
        assert!((adj.value() - range).abs() < 1.0, "must scroll to the end");

        // Same for the preview: the live pipeline renders on a background
        // thread, so pump until the text lands (or time out).
        let preview_view = editor
            .split
            .end_child()
            .and_then(|w| w.downcast::<gtk::ScrolledWindow>().ok())
            .and_then(|s| s.child())
            .and_then(|w| w.downcast::<gtk::TextView>().ok())
            .expect("preview scrolled window");
        let preview_buffer = preview_view.buffer();
        editor.request_preview_immediate();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while preview_buffer
            .text(
                &preview_buffer.start_iter(),
                &preview_buffer.end_iter(),
                true,
            )
            .is_empty()
        {
            assert!(
                std::time::Instant::now() < deadline,
                "async preview must land within 10s"
            );
            ctx.iteration(false);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        for _ in 0..50 {
            ctx.iteration(false);
        }
        let preview_adj = preview_view.vadjustment().expect("preview vadjustment");
        let preview_range = preview_adj.upper() - preview_adj.page_size();
        println!(
            "preview view: upper={:.1} page={:.1} value={:.1} range={:.1}",
            preview_adj.upper(),
            preview_adj.page_size(),
            preview_adj.value(),
            preview_range
        );
        assert!(
            preview_adj.page_size() > 100.0,
            "preview viewport must have real height"
        );
        assert!(preview_range > 0.0, "long preview must be scrollable");
    }

    /// Manual check that the editor stays editable at the widget level:
    /// `cargo test --bin notas source_view_stays_editable -- --ignored --nocapture`.
    #[test]
    #[ignore = "manual probe: needs a display and visual inspection"]
    fn source_view_stays_editable() {
        gtk::init().expect("gtk init");
        adw::init().expect("adw init");
        let editor = build_editor(|_| {}, Rc::new(Cell::new(false)));
        assert!(
            editor.source_view.is_editable(),
            "source view must be editable"
        );
        assert!(
            editor.source_view.is_sensitive(),
            "source view must be sensitive"
        );
        assert_eq!(
            editor.editor_mode(),
            crate::ui::protocol::EditorMode::Split,
            "split must be the default",
        );
        // Typing inserts into the source buffer even with no note open.
        editor.source_buffer.insert_at_cursor("hello");
        let text = editor.source_buffer.text(
            &editor.source_buffer.start_iter(),
            &editor.source_buffer.end_iter(),
            true,
        );
        assert_eq!(text, "hello");

        // The syntax highlighting scheme follows the libadwaita theme.
        let dark = adw::StyleManager::default().is_dark();
        let expected = if dark { "Adwaita-dark" } else { "Adwaita" };
        let scheme = editor
            .source_buffer
            .style_scheme()
            .map(|s| s.id().to_string());
        assert_eq!(scheme.as_deref(), Some(expected));
    }
}
