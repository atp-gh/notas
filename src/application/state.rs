//! Pure application state transitions shared by all frontends.

use crate::application::navigation::{ViewId, ViewMode};
use crate::core::model::NoteId;

/// Returns whether the editor differs from its last persisted snapshot.
pub fn editor_is_dirty(
    current_note: Option<NoteId>,
    current_title: &str,
    current_content: &str,
    saved_title: &str,
    saved_content: &str,
) -> bool {
    current_note.is_some() && (current_title != saved_title || current_content != saved_content)
}

/// Converts a built-in view selection into navigation state.
pub fn mode_for_view(view: ViewId) -> ViewMode {
    match view {
        ViewId::All => ViewMode::All,
        ViewId::Unfiled => ViewMode::Unfiled,
        ViewId::Trash => ViewMode::Trash,
    }
}

#[cfg(test)]
mod tests {
    use crate::core::model::NoteId;

    use super::*;

    #[test]
    fn editor_is_dirty_requires_a_selected_note() {
        assert!(!editor_is_dirty(None, "changed", "body", "", ""));
        assert!(editor_is_dirty(Some(NoteId(1)), "changed", "body", "", ""));
    }

    #[test]
    fn mode_for_view_maps_builtin_views() {
        assert_eq!(mode_for_view(ViewId::All), ViewMode::All);
        assert_eq!(mode_for_view(ViewId::Unfiled), ViewMode::Unfiled);
        assert_eq!(mode_for_view(ViewId::Trash), ViewMode::Trash);
    }
}
