//! Keymap overrides and chord dispatch.

use super::*;

impl AppState {
    pub fn load_keymap_overrides(&mut self) {
        let path = self
            .config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("keymaps.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(value) => {
                if let Some(base) = value.get("base").and_then(serde_json::Value::as_str) {
                    let id = KeymapId::from_session(base);
                    if id != self.settings.keymap {
                        self.settings.keymap = id;
                        self.session.keymap = id.session_value().into();
                        self.config_dirty = true;
                    }
                }
                self.keymap_overrides = Some(value);
                self.rebuild_keymap();
                self.status("keymap overrides loaded");
            }
            Err(error) => eprintln!("keymaps.json: {error}"),
        }
    }

    /// Resolves a host key through the active keymap. Overlay modules must run
    /// before this; plain editor keys and text stay untouched when no binding
    /// matches.
    pub fn keymap_key(&mut self, key: Key, modifiers: igui_core::Modifiers) -> bool {
        let scope = match self.focus {
            Focus::Terminal => Scope::Terminal,
            Focus::Editor => Scope::Editor,
            Focus::Find | Focus::Replace => Scope::Global,
        };
        if !self.keymap_pending.is_empty()
            && self.keymap_pending_at.elapsed() >= Duration::from_millis(900)
        {
            self.keymap_pending.clear();
        }
        let chord = KeyChord::from_event(key, modifiers, cfg!(target_os = "macos"));
        self.keymap_pending.push(chord);
        self.keymap_pending_at = Instant::now();
        match self.keymap.resolve(scope, &self.keymap_pending) {
            Resolution::Command(action) => {
                self.keymap_pending.clear();
                self.run(action);
                true
            }
            Resolution::Prefix => true,
            Resolution::None => {
                self.keymap_pending.clear();
                false
            }
        }
    }

    pub fn clear_keymap_pending(&mut self) {
        self.keymap_pending.clear();
        self.keymap_pending_at = Instant::now();
    }
}
