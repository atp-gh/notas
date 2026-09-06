//! Minimal i18n hook.
//!
//! All user-visible strings are marked with `tr!` so that switching to
//! gettext later is a mechanical change (swap the macro body for a call
//! into the gettext bindings, e.g. `gettextrs::gettext`).

#[macro_export]
macro_rules! tr {
    ($s:literal) => {
        $s
    };
}
