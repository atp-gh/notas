//! The editor pane: GtkSourceView-based Markdown editor, a rendered
//! preview, and a find/replace bar.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use libadwaita as adw;
use sourceview5::prelude::*;

use crate::editor::preview::{PreviewTags, apply_scheme, render_markdown};
use crate::ui::protocol::AppMsg;

mod preview;

pub struct Editor {
    pub source_view: sourceview5::View,
    pub source_buffer: sourceview5::Buffer,
    pub preview_buffer: gtk::TextBuffer,
    pub stack: gtk::Stack,
    pub search_bar: gtk::SearchBar,
    pub search_entry: gtk::SearchEntry,
    pub search_context: sourceview5::SearchContext,
    search_settings: sourceview5::SearchSettings,
    preview_tags: Rc<PreviewTags>,
}

impl Editor {
    pub fn render_preview(&self) {
        let md = self.source_buffer.text(
            &self.source_buffer.start_iter(),
            &self.source_buffer.end_iter(),
            true,
        );
        render_markdown(&self.preview_buffer, &self.preview_tags, &md);
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
        source_buffer.connect_changed(move |_| {
            if !suppress.get() {
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

    let preview_buffer = gtk::TextBuffer::new(None);
    let preview_tags = Rc::new(PreviewTags::new(&preview_buffer));

    // Keep the preview colors and the syntax highlighting scheme in sync
    // with the libadwaita theme. GtkSourceView defaults to a light scheme
    // and does not switch it automatically, so apply it ourselves.
    let scheme_manager = sourceview5::StyleSchemeManager::new();
    {
        let style_manager = adw::StyleManager::default();
        let dark = style_manager.is_dark();
        let accent = style_manager.accent_color_rgba();
        preview_tags.apply_theme(dark, &accent);
        apply_scheme(&scheme_manager, &source_buffer, dark);

        let tags = preview_tags.clone();
        let sm = style_manager.clone();
        let buf = source_buffer.clone();
        sm.clone().connect_dark_notify(move |_| {
            let dark = sm.is_dark();
            tags.apply_theme(dark, &sm.accent_color_rgba());
            apply_scheme(&scheme_manager, &buf, dark);
        });

        let tags = preview_tags.clone();
        let sm = style_manager;
        sm.clone().connect_accent_color_rgba_notify(move |_| {
            tags.apply_theme(sm.is_dark(), &sm.accent_color_rgba());
        });
    }

    let preview_view = gtk::TextView::new();
    preview_view.set_buffer(Some(&preview_buffer));
    preview_view.set_editable(false);
    preview_view.set_cursor_visible(false);
    preview_view.set_wrap_mode(gtk::WrapMode::WordChar);
    preview_view.set_left_margin(8);
    preview_view.set_right_margin(8);
    preview_view.set_top_margin(8);
    preview_view.set_bottom_margin(8);

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

    let preview_scroll = gtk::ScrolledWindow::new();
    preview_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    preview_scroll.set_child(Some(&preview_view));
    preview_scroll.set_vexpand(true);

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.add_named(&source_scroll, Some("source"));
    stack.add_named(&preview_scroll, Some("preview"));
    stack.set_visible_child_name("source");

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
        preview_buffer,
        stack,
        search_bar,
        search_entry,
        search_context,
        search_settings,
        preview_tags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        pane.append(&editor.stack);
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

        // Same for the preview: give it content, show it, and check its range.
        editor.render_preview();
        editor.stack.set_visible_child_name("preview");
        for _ in 0..50 {
            ctx.iteration(false);
        }
        let preview_view = editor
            .stack
            .child_by_name("preview")
            .and_then(|w| w.downcast::<gtk::ScrolledWindow>().ok())
            .and_then(|s| s.child())
            .and_then(|w| w.downcast::<gtk::TextView>().ok())
            .expect("preview scrolled window");
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
            editor.stack.visible_child_name().as_deref(),
            Some("source"),
            "stack must show the source view by default"
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
