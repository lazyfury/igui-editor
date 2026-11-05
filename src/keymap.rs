//! Keymap profiles and pure chord resolution.
//!
//! Bindings live here; `AppState` owns the pending-sequence state and executes
//! the returned [`Action`]. Overlay keys stay in their own modules and run
//! before this layer, so a completion popup or find field always wins.
//!
//! The profile format is platform-neutral at the `primary` level: `primary` is
//! Meta on macOS and Ctrl elsewhere. `ctrl`/`meta` remain available for literal
//! platform chords.

use igui_core::{Key, Modifiers};

use crate::state::{Action, Panel};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum KeymapId {
    #[default]
    VsCode,
    JetBrains,
    Vim,
}

impl KeymapId {
    pub const ALL: [KeymapId; 3] = [KeymapId::VsCode, KeymapId::JetBrains, KeymapId::Vim];

    pub const fn name(self) -> &'static str {
        match self {
            KeymapId::VsCode => "VS Code",
            KeymapId::JetBrains => "JetBrains",
            KeymapId::Vim => "Vim",
        }
    }

    pub const fn session_value(self) -> &'static str {
        match self {
            KeymapId::VsCode => "vscode",
            KeymapId::JetBrains => "idea",
            KeymapId::Vim => "vim",
        }
    }

    pub fn from_session(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "idea" | "jetbrains" | "intellij" => KeymapId::JetBrains,
            "vim" => KeymapId::Vim,
            _ => KeymapId::VsCode,
        }
    }

    pub fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct KeyChord {
    pub key: Key,
    pub primary: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl KeyChord {
    /// Normalizes a host event. The primary modifier is Cmd on macOS and Ctrl
    /// elsewhere; literal Ctrl/Meta remain distinct on macOS.
    pub fn from_event(key: Key, modifiers: Modifiers, macos: bool) -> Self {
        let (primary, ctrl, meta) = if macos {
            (modifiers.meta, modifiers.ctrl, false)
        } else {
            (modifiers.ctrl, false, modifiers.meta)
        };
        Self {
            key: normalize_key(key),
            primary,
            ctrl,
            alt: modifiers.alt,
            shift: modifiers.shift,
            meta,
        }
    }

    /// Parses one `primary+shift+p` style token. Used by the built-in profiles
    /// and the user keymap file.
    pub fn parse(value: &str) -> Option<Self> {
        let mut primary = false;
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut meta = false;
        let mut key = None;
        for part in value
            .split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            match part.to_ascii_lowercase().as_str() {
                "primary" | "cmd" | "command" => primary = true,
                // On non-macOS `primary` is Ctrl, so a literal `ctrl+` chord
                // normalizes to the same host bit. On macOS Ctrl is distinct.
                "ctrl" | "control" => {
                    if cfg!(target_os = "macos") {
                        ctrl = true;
                    } else {
                        primary = true;
                    }
                }
                "alt" | "option" | "opt" => alt = true,
                "shift" => shift = true,
                "meta" | "super" | "win" => meta = true,
                other => {
                    key = Some(parse_key(other)?);
                }
            }
        }
        let mut key = key?;
        // A literal uppercase token means Shift+key; the host reports shifted
        // characters that way too.
        if let Key::Character(ch) = key {
            if ch.is_ascii_uppercase() {
                shift = true;
                key = Key::Character(ch.to_ascii_lowercase());
            }
        }
        Some(Self {
            key,
            primary,
            ctrl,
            alt,
            shift,
            meta,
        })
    }

    /// Human label for menus. macOS uses the compact symbol form (`⇧⌘M`);
    /// other platforms spell modifiers out (`Ctrl+Shift+M`).
    pub fn label(self) -> String {
        let macos = cfg!(target_os = "macos");
        let mut parts: Vec<&str> = Vec::new();
        if macos {
            // Conventional symbol order: ⌃⌥⇧⌘.
            if self.ctrl {
                parts.push("⌃");
            }
            if self.alt {
                parts.push("⌥");
            }
            if self.shift {
                parts.push("⇧");
            }
            if self.meta {
                parts.push("Meta");
            }
            if self.primary {
                parts.push("⌘");
            }
        } else {
            if self.primary {
                parts.push("Ctrl");
            }
            if self.meta {
                parts.push("Meta");
            }
            if self.ctrl {
                parts.push("Ctrl");
            }
            if self.alt {
                parts.push("Alt");
            }
            if self.shift {
                parts.push("Shift");
            }
        }
        let key = key_label(self.key);
        let key = if macos && key.chars().count() == 1 {
            key.to_uppercase()
        } else {
            key
        };
        if macos {
            format!("{}{key}", parts.concat())
        } else if parts.is_empty() {
            key
        } else {
            format!("{}+{key}", parts.join("+"))
        }
    }
}

fn normalize_key(key: Key) -> Key {
    match key {
        Key::Character(ch) => Key::Character(ch.to_ascii_lowercase()),
        other => other,
    }
}

fn key_label(key: Key) -> String {
    match key {
        Key::Character(ch) => match ch {
            ' ' => "Space".into(),
            '\t' => "Tab".into(),
            _ => ch.to_string(),
        },
        Key::Enter => "Enter".into(),
        Key::Escape => "Esc".into(),
        Key::Backspace => "Backspace".into(),
        Key::Delete => "Delete".into(),
        Key::Tab => "Tab".into(),
        Key::Space => "Space".into(),
        Key::Home => "Home".into(),
        Key::End => "End".into(),
        Key::ArrowUp => "↑".into(),
        Key::ArrowDown => "↓".into(),
        Key::ArrowLeft => "←".into(),
        Key::ArrowRight => "→".into(),
        Key::F1 => "F1".into(),
        Key::F2 => "F2".into(),
        Key::F3 => "F3".into(),
        Key::F4 => "F4".into(),
        Key::F5 => "F5".into(),
        Key::F6 => "F6".into(),
        Key::F7 => "F7".into(),
        Key::F8 => "F8".into(),
        Key::F9 => "F9".into(),
        Key::F10 => "F10".into(),
        Key::F11 => "F11".into(),
        Key::F12 => "F12".into(),
    }
}

fn parse_key(value: &str) -> Option<Key> {
    let value = value.to_ascii_lowercase();
    let key = match value.as_str() {
        "enter" | "return" => Key::Enter,
        "escape" | "esc" => Key::Escape,
        "backspace" | "back" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "tab" => Key::Tab,
        "space" => Key::Space,
        "home" => Key::Home,
        "end" => Key::End,
        "up" | "arrowup" => Key::ArrowUp,
        "down" | "arrowdown" => Key::ArrowDown,
        "left" | "arrowleft" => Key::ArrowLeft,
        "right" | "arrowright" => Key::ArrowRight,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        other => {
            let mut chars = other.chars();
            let ch = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            Key::Character(ch)
        }
    };
    Some(key)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    Global,
    Editor,
    Terminal,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Resolution {
    Command(Action),
    Prefix,
    None,
}

#[derive(Clone, Debug)]
struct Binding {
    scope: Scope,
    sequence: Vec<KeyChord>,
    action: Action,
}

pub struct Keymap {
    pub id: KeymapId,
    bindings: Vec<Binding>,
}

impl Keymap {
    pub fn for_id(id: KeymapId) -> Self {
        let mut keymap = Self {
            id,
            bindings: Vec::new(),
        };
        keymap.add_global();
        keymap.add_movement();
        keymap.add_terminal();
        match id {
            KeymapId::VsCode => keymap.add_vscode(),
            KeymapId::JetBrains => keymap.add_ide(),
            KeymapId::Vim => {}
        }
        keymap
    }

    fn bind(&mut self, scope: Scope, sequence: &str, action: Action) {
        let Some(sequence) = parse_sequence(sequence) else {
            return;
        };
        self.bindings
            .retain(|binding| !(binding.scope == scope && binding.sequence == sequence));
        self.bindings.push(Binding {
            scope,
            sequence,
            action,
        });
    }

    fn add_global(&mut self) {
        let global = [
            ("primary+n", Action::New),
            ("primary+shift+n", Action::NewFile),
            ("primary+o", Action::Open),
            ("primary+shift+o", Action::OpenFolder),
            ("primary+s", Action::Save),
            ("primary+shift+s", Action::SaveAs),
            ("primary+w", Action::CloseTab),
            ("primary+z", Action::Undo),
            ("primary+shift+z", Action::Redo),
            ("primary+x", Action::Cut),
            ("primary+c", Action::Copy),
            ("primary+v", Action::Paste),
            ("primary+a", Action::SelectAll),
            ("primary+f", Action::Find),
            ("primary+alt+f", Action::FindReplace),
            ("alt+shift+f", Action::FormatDocument),
            ("primary+shift+f", Action::ProjectSearch),
            ("primary+g", Action::FindNext),
            ("primary+shift+g", Action::FindPrevious),
            ("primary+p", Action::QuickOpen),
            ("primary+,", Action::Settings),
            ("primary+b", Action::ToggleSidebar),
            ("primary+alt+o", Action::ToggleOutline),
            ("primary+shift+l", Action::ToggleTheme),
            ("primary+shift+m", Action::CycleView),
            ("primary+shift+y", Action::ToggleScrollSync),
            ("primary+i", Action::LspCompletion),
            ("ctrl+space", Action::LspCompletion),
            ("f1", Action::LspHover),
            ("primary+shift+i", Action::LspHover),
            ("f12", Action::LspDefinition),
            ("primary+shift+d", Action::LspDefinition),
            ("shift+f12", Action::LspReferences),
            ("f2", Action::LspRename),
            ("alt+z", Action::ToggleSoftWrap),
            ("ctrl+`", Action::ToggleTerminal),
            ("primary+alt+p", Action::ShowPanel(Panel::Problems)),
        ];
        for (sequence, action) in global {
            self.bind(Scope::Global, sequence, action);
        }
        // Direct tab access: `primary+1..8` selects by index, `primary+9` the
        // last tab (VS Code convention). Ctrl+Tab is a literal Ctrl on macOS
        // (Cmd+Tab belongs to the OS) and the primary chord elsewhere.
        for slot in 1..=9 {
            self.bind(
                Scope::Global,
                &format!("primary+{slot}"),
                Action::SelectTab(slot),
            );
        }
        if cfg!(target_os = "macos") {
            self.bind(Scope::Global, "ctrl+tab", Action::NextTab);
            self.bind(Scope::Global, "ctrl+shift+tab", Action::PreviousTab);
        } else {
            self.bind(Scope::Global, "primary+tab", Action::NextTab);
            self.bind(Scope::Global, "primary+shift+tab", Action::PreviousTab);
        }
    }

    /// Movement chords shared by every profile. Editor-scoped so they cannot
    /// move the document while a field or the terminal owns focus.
    fn add_movement(&mut self) {
        self.bind(Scope::Editor, "primary+left", Action::MoveLineStart(false));
        self.bind(Scope::Editor, "primary+right", Action::MoveLineEnd(false));
        self.bind(Scope::Editor, "primary+up", Action::MoveDocStart(false));
        self.bind(Scope::Editor, "primary+down", Action::MoveDocEnd(false));
        self.bind(
            Scope::Editor,
            "primary+shift+left",
            Action::MoveLineStart(true),
        );
        self.bind(
            Scope::Editor,
            "primary+shift+right",
            Action::MoveLineEnd(true),
        );
        self.bind(
            Scope::Editor,
            "primary+shift+up",
            Action::MoveDocStart(true),
        );
        self.bind(
            Scope::Editor,
            "primary+shift+down",
            Action::MoveDocEnd(true),
        );
        self.bind(Scope::Editor, "primary+l", Action::GotoLine);
        // macOS keeps Ctrl+G literal for Go to Line; elsewhere Ctrl+G is Find
        // Next (the global `primary+g` binding).
        if cfg!(target_os = "macos") {
            self.bind(Scope::Editor, "ctrl+g", Action::GotoLine);
        }
    }

    /// Clipboard chords that do not collide with the shell's Ctrl letters on
    /// Linux/Windows (Ctrl+Shift+C/V are the conventional terminal copy/paste).
    fn add_terminal(&mut self) {
        self.bind(Scope::Terminal, "primary+shift+c", Action::Copy);
        self.bind(Scope::Terminal, "primary+shift+v", Action::Paste);
        self.bind(Scope::Terminal, "primary+shift+a", Action::SelectAll);
    }

    fn add_vscode(&mut self) {
        self.bind(Scope::Editor, "primary+shift+k", Action::DeleteLine);
        self.bind(Scope::Editor, "primary+d", Action::SelectNextOccurrence);
        self.bind(Scope::Editor, "primary+/", Action::ToggleComment);
        self.bind(Scope::Editor, "alt+up", Action::MoveLineUp);
        self.bind(Scope::Editor, "alt+down", Action::MoveLineDown);
        self.bind(Scope::Editor, "shift+alt+down", Action::DuplicateLine);
        if cfg!(target_os = "macos") {
            self.bind(Scope::Editor, "alt+backspace", Action::DeleteWordBackward);
            self.bind(Scope::Editor, "alt+delete", Action::DeleteWordForward);
        } else {
            self.bind(
                Scope::Editor,
                "primary+backspace",
                Action::DeleteWordBackward,
            );
            self.bind(Scope::Editor, "primary+delete", Action::DeleteWordForward);
        }
    }

    fn add_ide(&mut self) {
        self.bind(Scope::Editor, "primary+d", Action::DuplicateLine);
        // JetBrains' "add selection for next occurrence".
        self.bind(Scope::Editor, "alt+j", Action::SelectNextOccurrence);
        self.bind(Scope::Editor, "primary+/", Action::ToggleComment);
        if cfg!(target_os = "macos") {
            self.bind(Scope::Editor, "primary+backspace", Action::DeleteLine);
            self.bind(Scope::Editor, "alt+backspace", Action::DeleteWordBackward);
            self.bind(Scope::Editor, "alt+delete", Action::DeleteWordForward);
        } else {
            self.bind(Scope::Editor, "primary+y", Action::DeleteLine);
            self.bind(
                Scope::Editor,
                "primary+backspace",
                Action::DeleteWordBackward,
            );
            self.bind(Scope::Editor, "primary+delete", Action::DeleteWordForward);
        }
        self.bind(Scope::Editor, "primary+shift+up", Action::MoveLineUp);
        self.bind(Scope::Editor, "primary+shift+down", Action::MoveLineDown);
    }

    /// Applies data-only overrides (`<config>/keymaps.json`). Unknown command
    /// ids and malformed chords are ignored so a broken file cannot brick input.
    pub fn apply_overrides(&mut self, value: &serde_json::Value) {
        let Some(bindings) = value.get("bindings").and_then(serde_json::Value::as_object) else {
            return;
        };
        for (scope_name, entries) in bindings {
            let scope = match scope_name.to_ascii_lowercase().as_str() {
                "global" | "app" => Scope::Global,
                "editor" => Scope::Editor,
                "terminal" => Scope::Terminal,
                _ => continue,
            };
            let Some(entries) = entries.as_object() else {
                continue;
            };
            for (sequence, action) in entries {
                let Some(sequence) = parse_sequence(sequence) else {
                    continue;
                };
                self.bindings
                    .retain(|binding| !(binding.scope == scope && binding.sequence == sequence));
                if action.is_null() {
                    continue;
                }
                let Some(id) = action.as_str() else {
                    continue;
                };
                if let Some(action) = Action::from_id(id) {
                    self.bindings.push(Binding {
                        scope,
                        sequence,
                        action,
                    });
                }
            }
        }
    }

    pub fn resolve(&self, scope: Scope, sequence: &[KeyChord]) -> Resolution {
        let mut prefix = false;
        let mut global_exact = None;
        for binding in &self.bindings {
            if !applies(binding.scope, scope) {
                continue;
            }
            if binding.sequence.len() < sequence.len()
                || binding.sequence[..sequence.len()] != *sequence
            {
                continue;
            }
            if binding.sequence.len() == sequence.len() {
                if binding.scope == scope {
                    return Resolution::Command(binding.action.clone());
                }
                global_exact.get_or_insert_with(|| binding.action.clone());
            } else {
                prefix = true;
            }
        }
        if let Some(action) = global_exact {
            return Resolution::Command(action);
        }
        if prefix {
            Resolution::Prefix
        } else {
            Resolution::None
        }
    }

    /// Display label for the binding that runs `action` (global first, then the
    /// first scope-specific binding). Menus and the welcome screen use this so
    /// they cannot drift from the active profile.
    pub fn shortcut_label(&self, action: &Action) -> Option<String> {
        let binding = self
            .bindings
            .iter()
            .find(|binding| binding.action == *action && binding.scope == Scope::Global)
            .or_else(|| {
                self.bindings
                    .iter()
                    .find(|binding| binding.action == *action)
            })?;
        Some(
            binding
                .sequence
                .iter()
                .map(|chord| chord.label())
                .collect::<Vec<_>>()
                .join(" "),
        )
    }
}

fn applies(binding: Scope, requested: Scope) -> bool {
    binding == Scope::Global || binding == requested
}

fn parse_sequence(value: &str) -> Option<Vec<KeyChord>> {
    let chords: Vec<KeyChord> = value
        .split_whitespace()
        .map(KeyChord::parse)
        .collect::<Option<_>>()?;
    (!chords.is_empty()).then_some(chords)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(value: &str) -> KeyChord {
        KeyChord::parse(value).expect("valid chord")
    }

    #[test]
    fn primary_is_platform_aware() {
        let mac = KeyChord::from_event(
            Key::Character('p'),
            Modifiers {
                meta: true,
                ..Modifiers::NONE
            },
            true,
        );
        let other = KeyChord::from_event(
            Key::Character('p'),
            Modifiers {
                ctrl: true,
                ..Modifiers::NONE
            },
            false,
        );
        assert_eq!(mac, other);
        assert!(mac.primary);
    }

    #[test]
    fn literal_ctrl_resolves_on_every_platform() {
        let macos = cfg!(target_os = "macos");
        let ctrl_space = KeyChord::from_event(
            Key::Space,
            Modifiers {
                ctrl: true,
                ..Modifiers::NONE
            },
            macos,
        );
        assert_eq!(KeyChord::parse("ctrl+space"), Some(ctrl_space));
        assert_eq!(
            Keymap::for_id(KeymapId::VsCode).resolve(Scope::Editor, &[ctrl_space]),
            Resolution::Command(Action::LspCompletion)
        );

        let ctrl_backtick = KeyChord::from_event(
            Key::Character('`'),
            Modifiers {
                ctrl: true,
                ..Modifiers::NONE
            },
            macos,
        );
        assert_eq!(KeyChord::parse("ctrl+`"), Some(ctrl_backtick));
        assert_eq!(
            Keymap::for_id(KeymapId::VsCode).resolve(Scope::Global, &[ctrl_backtick]),
            Resolution::Command(Action::ToggleTerminal)
        );
    }

    #[test]
    fn movement_chords_are_editor_scoped_and_select_with_shift() {
        let keymap = Keymap::for_id(KeymapId::VsCode);
        assert_eq!(
            keymap.resolve(Scope::Editor, &[chord("primary+left")]),
            Resolution::Command(Action::MoveLineStart(false))
        );
        assert_eq!(
            keymap.resolve(Scope::Editor, &[chord("primary+shift+down")]),
            Resolution::Command(Action::MoveDocEnd(true))
        );
        assert_eq!(
            keymap.resolve(Scope::Terminal, &[chord("primary+left")]),
            Resolution::None
        );
    }

    #[test]
    fn goto_line_chords_follow_the_host() {
        let keymap = Keymap::for_id(KeymapId::VsCode);
        assert_eq!(
            keymap.resolve(Scope::Editor, &[chord("primary+l")]),
            Resolution::Command(Action::GotoLine)
        );
        if cfg!(target_os = "macos") {
            let ctrl_g = KeyChord::from_event(
                Key::Character('g'),
                Modifiers {
                    ctrl: true,
                    ..Modifiers::NONE
                },
                true,
            );
            assert_eq!(ctrl_g, chord("ctrl+g"));
            assert_eq!(
                keymap.resolve(Scope::Editor, &[ctrl_g]),
                Resolution::Command(Action::GotoLine)
            );
        } else {
            // A literal Ctrl+G stays Find Next off macOS.
            assert_eq!(
                keymap.resolve(Scope::Editor, &[chord("primary+g")]),
                Resolution::Command(Action::FindNext)
            );
        }
    }

    #[test]
    fn profiles_resolve_global_and_editor_bindings() {
        let keymap = Keymap::for_id(KeymapId::VsCode);
        assert_eq!(
            keymap.resolve(Scope::Editor, &[chord("primary+s")]),
            Resolution::Command(Action::Save)
        );
        assert_eq!(
            keymap.resolve(Scope::Editor, &[chord("primary+shift+k")]),
            Resolution::Command(Action::DeleteLine)
        );
        assert_eq!(
            keymap.resolve(Scope::Terminal, &[chord("primary+shift+k")]),
            Resolution::None
        );
        assert_eq!(
            keymap.resolve(Scope::Editor, &[chord("primary+d")]),
            Resolution::Command(Action::SelectNextOccurrence)
        );
        // JetBrains keeps primary+D for duplicate and uses Alt+J here.
        let ide = Keymap::for_id(KeymapId::JetBrains);
        assert_eq!(
            ide.resolve(Scope::Editor, &[chord("primary+d")]),
            Resolution::Command(Action::DuplicateLine)
        );
        assert_eq!(
            ide.resolve(Scope::Editor, &[chord("alt+j")]),
            Resolution::Command(Action::SelectNextOccurrence)
        );
    }

    #[test]
    fn user_overrides_replace_and_unbind_bindings() {
        let mut keymap = Keymap::for_id(KeymapId::VsCode);
        keymap.apply_overrides(&serde_json::json!({
            "bindings": {
                "editor": {
                    "primary+shift+k": "edit.duplicateLine",
                    "alt+up": null
                }
            }
        }));
        assert_eq!(
            keymap.resolve(Scope::Editor, &[chord("primary+shift+k")]),
            Resolution::Command(Action::DuplicateLine)
        );
        assert_eq!(
            keymap.resolve(Scope::Editor, &[chord("alt+up")]),
            Resolution::None
        );
    }

    #[test]
    fn editor_binding_wins_over_global_and_prefixes_are_reported() {
        let keymap = Keymap::for_id(KeymapId::VsCode);
        assert_eq!(
            keymap.resolve(Scope::Editor, &[chord("alt+up")]),
            Resolution::Command(Action::MoveLineUp)
        );
        let keymap = Keymap::for_id(KeymapId::Vim);
        assert_eq!(
            keymap.resolve(Scope::Editor, &[chord("primary+s")]),
            Resolution::Command(Action::Save)
        );
        assert_eq!(
            keymap.resolve(Scope::Editor, &[chord("g")]),
            Resolution::None
        );
    }
    #[test]
    fn shortcut_labels_follow_the_active_profile_and_platform() {
        let vscode = Keymap::for_id(KeymapId::VsCode);
        let save = vscode.shortcut_label(&Action::Save).expect("save binding");
        if cfg!(target_os = "macos") {
            assert!(save.contains('⌘'), "{save}");
        } else {
            assert!(save.contains("Ctrl"), "{save}");
        }
        // Editor-scope bindings are still discoverable in the welcome screen.
        assert!(vscode
            .shortcut_label(&Action::DuplicateLine)
            .is_some_and(|label| label.contains('↓')));
        // The label follows the profile: JetBrains duplicates with primary+D.
        let idea = Keymap::for_id(KeymapId::JetBrains);
        let duplicate = idea
            .shortcut_label(&Action::DuplicateLine)
            .expect("idea duplicate binding");
        assert!(duplicate.ends_with('D'), "{duplicate}");
    }

    #[test]
    fn terminal_clipboard_chords_are_terminal_scoped() {
        let keymap = Keymap::for_id(KeymapId::VsCode);
        assert_eq!(
            keymap.resolve(Scope::Terminal, &[chord("primary+shift+c")]),
            Resolution::Command(Action::Copy)
        );
        assert_eq!(
            keymap.resolve(Scope::Editor, &[chord("primary+shift+c")]),
            Resolution::None
        );
        assert_eq!(
            keymap.resolve(Scope::Terminal, &[chord("primary+shift+a")]),
            Resolution::Command(Action::SelectAll)
        );
    }

    #[test]
    fn tab_chords_select_and_cycle() {
        let keymap = Keymap::for_id(KeymapId::VsCode);
        let sequence = |value: &str| parse_sequence(value).expect("chord");
        assert_eq!(
            keymap.resolve(Scope::Global, &sequence("primary+2")),
            Resolution::Command(Action::SelectTab(2))
        );
        assert_eq!(
            keymap.resolve(Scope::Global, &sequence("primary+9")),
            Resolution::Command(Action::SelectTab(9))
        );
        let next = if cfg!(target_os = "macos") {
            "ctrl+tab"
        } else {
            "primary+tab"
        };
        assert_eq!(
            keymap.resolve(Scope::Global, &sequence(next)),
            Resolution::Command(Action::NextTab)
        );
    }
}
