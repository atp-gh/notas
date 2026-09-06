//! Status-bar presentation helpers.

use crate::tr;

/// Text shown for the persistent synchronization indicator.
pub(crate) fn sync_indicator_text(last_synced_at: &str) -> String {
    if last_synced_at.is_empty() {
        tr!("Never synced").to_string()
    } else {
        format!("{} {last_synced_at}", tr!("Last synced"))
    }
}
