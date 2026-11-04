//! Confirmation modal resolution.

use super::*;

impl AppState {
    /// Applies a confirmation-modal choice.
    pub fn resolve_modal(&mut self, button: ModalButton) {
        let Some(modal) = self.modal else {
            return;
        };
        if let Modal::ReplaceInFiles { .. } = modal {
            match button {
                ModalButton::Cancel => {
                    self.pending_replace = None;
                    self.modal = None;
                }
                ModalButton::Apply => {
                    let Some(job) = self.pending_replace.take() else {
                        self.modal = None;
                        return;
                    };
                    let applied = replace::apply(job.plans);
                    self.modal = None;
                    let mut message = format!(
                        "replaced {} in {} file(s)",
                        applied.occurrences, applied.files
                    );
                    if !applied.errors.is_empty() {
                        message.push_str(&format!("; {} skipped", applied.errors.len()));
                        if let Some(first) = applied.errors.first() {
                            message.push_str(&format!(" ({first})"));
                        }
                    }
                    self.status(message);
                    self.search_query_changed();
                    self.dirty = true;
                }
                _ => {}
            }
            return;
        }
        if let Modal::Quit { .. } = modal {
            self.modal = None;
            if button == ModalButton::Discard {
                self.shutdown_children();
            }
            crate::macos::reply_quit(button == ModalButton::Discard);
            self.dirty = true;
            return;
        }
        if let Modal::ExternalChange { tab } = modal {
            self.modal = None;
            if button == ModalButton::Reload {
                self.reload_tab(tab, true);
            } else {
                self.status("kept the buffer");
            }
            self.dirty = true;
            return;
        }
        if let Modal::CloseFolder { .. } = modal {
            self.modal = None;
            if button == ModalButton::Discard {
                self.close_folder_now();
            }
            self.dirty = true;
            return;
        }
        if let Modal::CloseTabs { keep, from } = modal {
            self.modal = None;
            if button == ModalButton::Discard {
                self.close_tabs_now(keep, from);
            }
            self.dirty = true;
            return;
        }
        let Some(Modal::CloseDirtyTab { tab }) = self.modal else {
            return;
        };
        self.modal = None;
        match button {
            ModalButton::Cancel => {}
            ModalButton::Discard => self.close_tab_now(tab),
            ModalButton::Save => {
                let has_path = self
                    .ws
                    .tabs
                    .get(tab)
                    .is_some_and(|doc| doc.path().is_some());
                if !has_path {
                    self.ws.select(tab);
                    self.tabs.reveal_active = true;
                    self.pending_dialog = Some(DialogRequest {
                        kind: DialogKind::SaveAs,
                        then_close: Some(tab),
                    });
                    return;
                }
                match self.ws.save_tab(tab) {
                    Ok(message) => {
                        self.note_saved();
                        self.status(message);
                        self.close_tab_now(tab);
                    }
                    Err(message) => self.error(message),
                }
            }
            ModalButton::Apply | ModalButton::Reload => {}
        }
        self.dirty = true;
    }
}
