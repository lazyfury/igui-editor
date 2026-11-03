//! Per-document view mode and split layout.

use super::*;

impl AppState {
    // -- view mode -------------------------------------------------------

    pub fn view_of_active(&self) -> ViewMode {
        let Some(doc) = self.ws.active_doc() else {
            return ViewMode::Edit;
        };
        self.doc_views
            .get(&doc.id())
            .map(|ui| ui.view)
            .unwrap_or_else(|| ViewMode::for_path(doc.path()))
    }

    /// Split ratio for the active document (clamped 0.2–0.8).
    pub fn split_of_active(&self) -> f32 {
        self.ws
            .active_doc()
            .and_then(|doc| self.doc_views.get(&doc.id()))
            .map(|ui| ui.split_ratio)
            .unwrap_or(self.session.split)
            .clamp(0.2, 0.8)
    }

    pub fn set_view(&mut self, mode: ViewMode) {
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        let id = doc.id();
        let default_ratio = self.session.split.clamp(0.2, 0.8);
        self.doc_views
            .entry(id)
            .or_insert(DocUi {
                view: ViewMode::for_path(doc.path()),
                split_ratio: default_ratio,
            })
            .view = mode;
        self.sync_layout();
        self.status(format!("view: {}", mode.label()));
    }

    /// Sets the split ratio for the active document and remembers it globally.
    pub fn set_split_ratio(&mut self, ratio: f32) {
        let ratio = ratio.clamp(0.2, 0.8);
        if let Some(doc) = self.ws.active_doc() {
            let id = doc.id();
            let default_ratio = self.session.split.clamp(0.2, 0.8);
            let entry = self.doc_views.entry(id).or_insert(DocUi {
                view: ViewMode::for_path(doc.path()),
                split_ratio: default_ratio,
            });
            entry.split_ratio = ratio;
        }
        self.session.split = ratio;
        self.config_dirty = true;
        self.dirty = true;
    }

    /// Rebuild the shell when the pane structure the active document needs
    /// differs from the one currently mounted.
    pub fn sync_layout(&mut self) {
        let view = self.view_of_active();
        let docs = self.ws.active_doc().is_some_and(|doc| doc.is_docs_view());
        let ratio = self.split_of_active();
        let needs = docs != self.built_docs
            || view != self.built_view
            || (view == ViewMode::Split && (ratio - self.built_split).abs() > 0.001);
        if needs {
            self.needs_rebuild = true;
        }
    }
}
