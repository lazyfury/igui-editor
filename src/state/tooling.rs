//! Reusable "missing tool" prompt.
//!
//! Any native service can report a missing executable (language servers today,
//! formatters/Git tooling later). The prompt is copy-only: the editor never
//! installs software by itself, it just shows the command and lets the user
//! paste it into the terminal panel.

use std::time::Instant;

use super::*;
use crate::model::lsp::LspIssue;

/// One missing-tool prompt.
#[derive(Clone, Debug, Default)]
pub struct ToolingPrompt {
    /// Executable name, e.g. `gopls`.
    pub tool: String,
    /// Human language name, e.g. `Go`.
    pub language: String,
    /// Copyable install command (may be empty when the spec has no hint).
    pub command: String,
    pub note: Option<String>,
    /// One-line explanation shown under the title.
    pub detail: String,
    /// The command the editor tried to start.
    pub source: String,
    /// Set after COPY succeeds.
    pub copied: bool,
}

#[derive(Default)]
pub struct ToolingUi {
    pub prompt: Option<ToolingPrompt>,
    pub copied_at: Option<Instant>,
}

impl ToolingUi {
    pub fn show(&mut self, issue: LspIssue) {
        let detail = issue.detail();
        self.show_missing_tool(
            issue.tool,
            issue.language,
            issue.command,
            issue.note,
            detail,
            issue.source,
        );
    }

    /// Generic entry point: any native service (LSP, formatters, Git tooling)
    /// can report a missing executable with a copy-only install command.
    pub fn show_missing_tool(
        &mut self,
        tool: impl Into<String>,
        language: impl Into<String>,
        command: impl Into<String>,
        note: Option<String>,
        detail: impl Into<String>,
        source: impl Into<String>,
    ) {
        self.prompt = Some(ToolingPrompt {
            tool: tool.into(),
            language: language.into(),
            command: command.into(),
            note,
            detail: detail.into(),
            source: source.into(),
            copied: false,
        });
        self.copied_at = None;
    }

    pub fn close(&mut self) {
        self.prompt = None;
        self.copied_at = None;
    }

    /// Clears the "copied" confirmation after its short TTL.
    pub fn tick(&mut self) {
        if self
            .copied_at
            .is_some_and(|at| at.elapsed().as_millis() > 1_500)
        {
            self.copied_at = None;
            if let Some(prompt) = self.prompt.as_mut() {
                prompt.copied = false;
            }
        }
    }
}

impl AppState {
    /// Shows (or replaces) the missing-tool prompt and dismisses transient LSP
    /// popups so the user sees a single message.
    pub fn show_tool_prompt(&mut self, issue: LspIssue) {
        self.tooling.show(issue);
        self.problems.open = false;
        self.lsp.dismiss_transient();
        self.dirty = true;
    }

    pub fn close_tool_prompt(&mut self) {
        if self.tooling.prompt.is_some() {
            self.tooling.close();
            self.dirty = true;
        }
    }

    /// Copies the install command (never runs it).
    pub fn copy_tool_command(&mut self) {
        let Some(command) = self
            .tooling
            .prompt
            .as_ref()
            .map(|prompt| prompt.command.clone())
        else {
            return;
        };
        if command.is_empty() {
            self.status("this server has no install command");
            return;
        }
        self.set_clipboard(&command);
        if let Some(prompt) = self.tooling.prompt.as_mut() {
            prompt.copied = true;
        }
        self.tooling.copied_at = Some(Instant::now());
        self.status("install command copied");
        self.dirty = true;
    }

    /// Clears the start back-off so the next frame retries the server.
    pub fn retry_servers(&mut self) {
        self.lsp.retry();
        self.tooling.close();
        self.status("retrying language servers");
        self.dirty = true;
    }

    /// Single entry point for the settings row and the footer chip.
    pub fn set_lsp_enabled(&mut self, enabled: bool) {
        if self.settings.lsp == enabled {
            return;
        }
        self.settings.lsp = enabled;
        self.lsp.set_enabled(enabled);
        if !enabled {
            self.tooling.close();
            self.problems.open = false;
        }
        self.settings_changed();
        self.status(if enabled {
            "language servers enabled"
        } else {
            "language servers stopped"
        });
    }
}
