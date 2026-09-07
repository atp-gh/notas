//! The Appearance page of the settings dialog: the theme selector and the
//! editor rows (line numbers, status bar).

use libadwaita as adw;
use libadwaita::prelude::*;

use crate::application::config::{Settings, ThemeMode};
use crate::tr;
use crate::ui::AppMsg;

/// Build the "Appearance" page. Changing a row emits the matching `AppMsg`
/// with **immediate effect** (no Apply button), which the app applies to
/// the live UI and saves.
pub fn build_appearance_page<E>(settings: &Settings, emit: E) -> adw::PreferencesPage
where
    E: Fn(AppMsg) + 'static + Clone,
{
    let page = adw::PreferencesPage::new();
    page.set_title(tr!("Appearance"));
    page.set_icon_name(Some("applications-graphics-symbolic"));

    // --- appearance: theme ------------------------------------------------
    let theme_group = adw::PreferencesGroup::new();
    theme_group.set_title(tr!("Theme"));
    theme_group.set_description(Some(tr!(
        "Color scheme used for the whole app, including the editor."
    )));

    let model = gtk::StringList::new(&[tr!("Follow system"), tr!("Light"), tr!("Dark")]);
    let theme_row = adw::ComboRow::new();
    theme_row.set_title(tr!("Theme"));
    theme_row.set_model(Some(&model));
    theme_row.set_selected(settings.theme.mode.index());
    {
        let emit = emit.clone();
        theme_row.connect_selected_notify(move |row| {
            emit(AppMsg::ThemeChanged(ThemeMode::from_index(row.selected())));
        });
    }

    // --- editor: line numbers, status bar ---------------------------------
    let editor_group = adw::PreferencesGroup::new();
    editor_group.set_title(tr!("Editor"));

    let line_numbers_row = adw::SwitchRow::new();
    line_numbers_row.set_title(tr!("Show line numbers"));
    line_numbers_row.set_subtitle(tr!("Gutter on the left of the editor"));
    line_numbers_row.set_active(settings.editor.show_line_numbers);
    {
        let emit = emit.clone();
        line_numbers_row.connect_active_notify(move |row| {
            emit(AppMsg::ToggleLineNumbers(row.is_active()));
        });
    }

    let status_bar_row = adw::SwitchRow::new();
    status_bar_row.set_title(tr!("Show status bar"));
    status_bar_row.set_subtitle(tr!("Saved-state indicator at the bottom of the editor"));
    status_bar_row.set_active(settings.interface.show_status_bar);
    {
        status_bar_row.connect_active_notify(move |row| {
            emit(AppMsg::ToggleStatusBar(row.is_active()));
        });
    }

    theme_group.add(&theme_row);
    page.add(&theme_group);
    editor_group.add(&line_numbers_row);
    editor_group.add(&status_bar_row);
    page.add(&editor_group);

    page
}
