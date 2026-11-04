//! `Format Document`: indent normalization without reflowing code.
//!
//! The editor has no language formatter; what it can do safely is convert the
//! leading indentation to the configured unit. That is exactly the operation
//! that repairs a Godot script whose template tabs were mixed with spaces.

use super::*;
use crate::model::doc::IndentStyle;

impl AppState {
    /// Converts the active buffer's indentation to the configured style.
    pub fn format_active_document(&mut self) {
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        if doc.is_read_only() {
            self.status("read-only view");
            return;
        }
        let style = self.settings.indent_style();
        let width = self.settings.tab_width;
        let gdscript = self.language_name() == Some("gdscript");
        let Some(doc) = self.ws.active_doc_mut() else {
            return;
        };
        if doc.format_indentation(style, width, gdscript) {
            let label = match style {
                IndentStyle::Tabs => "tabs".to_string(),
                IndentStyle::Spaces(width) => format!("{width} spaces"),
            };
            self.status(format!("formatted: {label}"));
            self.find.dirty = true;
            self.dirty = true;
        } else {
            self.status("format: indentation already consistent");
        }
    }
}
