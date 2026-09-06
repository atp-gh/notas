//! GTK notebook tree rendering.

use notas::domain_notes::Notebook;

/// Rebuild a notebook tree from the core model list.
#[expect(
    deprecated,
    reason = "the existing TreeView backend remains until the ColumnView migration"
)]
pub(crate) fn rebuild_tree(store: &gtk::TreeStore, notebooks: &[Notebook]) {
    store.clear();
    let mut iters = std::collections::HashMap::new();
    for notebook in notebooks {
        let parent = notebook
            .parent_id
            .and_then(|parent_id| iters.get(&parent_id).cloned());
        let iter = store.insert_with_values(
            parent.as_ref(),
            None,
            &[(0, &notebook.id), (1, &notebook.name)],
        );
        iters.insert(notebook.id, iter);
    }
}
