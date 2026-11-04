//! The settings panel: a centered, square overlay (same pattern as the modal).
//!
//! Every row applies immediately. Values are persisted through the session by
//! `AppState`; the panel itself only paints values and dispatches adjustments.
//!
//! Keys: `↑`/`↓` select a row, `←`/`→` adjust, `Enter`/`Space` toggle/step,
//! `Esc` (or `⌘,`) closes. The pointer can press the `−`/`+` boxes or a row.

use igui_core::{Key, Modifiers, Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit::{self, RectExt};
use crate::settings::{Row, ROWS};
use crate::state::AppState;

const ROW_H: f32 = 34.0;
const PANEL_W: f32 = 560.0;
const HEADER_H: f32 = 56.0;
const FOOTER_H: f32 = 36.0;
const BUTTON: f32 = 26.0;

/// Panel + row + button geometry (one function for paint and hit tests).
pub struct Layout {
    pub panel: Rect,
    pub rows: Vec<Row>,
    pub rows_rect: Vec<Rect>,
    pub minus: Vec<Option<Rect>>,
    pub plus: Vec<Option<Rect>>,
}

pub fn layout(viewport: Rect) -> Layout {
    let height = HEADER_H + ROWS.len() as f32 * ROW_H + FOOTER_H;
    let panel = Rect::from_center_size(
        viewport.center(),
        Size::new(PANEL_W.min(viewport.width() - 40.0).max(280.0), height),
    );
    let mut rows_rect = Vec::with_capacity(ROWS.len());
    let mut minus = Vec::with_capacity(ROWS.len());
    let mut plus = Vec::with_capacity(ROWS.len());
    let mut y = panel.top() + HEADER_H;
    for row in ROWS {
        let rect = Rect::from_min_size(
            Vec2::new(panel.left() + 8.0, y),
            Size::new(panel.width() - 16.0, ROW_H),
        );
        rows_rect.push(rect);
        let steppable = matches!(
            row,
            Row::Theme | Row::FontSize | Row::TabWidth | Row::SidebarWidth | Row::Keymap
        );
        if steppable {
            let plus_rect = Rect::from_min_size(
                Vec2::new(rect.right() - BUTTON, rect.center().y - BUTTON * 0.5),
                Size::new(BUTTON, BUTTON),
            );
            let minus_rect = Rect::from_min_size(
                Vec2::new(plus_rect.left() - BUTTON - 6.0, plus_rect.top()),
                Size::new(BUTTON, BUTTON),
            );
            minus.push(Some(minus_rect));
            plus.push(Some(plus_rect));
        } else {
            minus.push(None);
            plus.push(None);
        }
        y += ROW_H;
    }
    Layout {
        panel,
        rows: ROWS.to_vec(),
        rows_rect,
        minus,
        plus,
    }
}

pub fn paint(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    if !state.settings_open {
        return;
    }
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let layout = layout(state.viewport.logical_rect());

    ctx.fill_rect(
        state.viewport.logical_rect(),
        igui_core::Color::new(0.0, 0.0, 0.0, 0.4),
    );
    kit::fill(ctx, layout.panel, p.raised);
    ctx.stroke_rect(layout.panel, 1.0, p.border);
    kit::hline(
        ctx,
        layout.panel.left(),
        layout.panel.right(),
        layout.panel.top(),
        p.accent,
    );

    kit::text(
        ctx,
        "Settings",
        layout.panel.left() + 16.0,
        layout.panel.top() + 28.0,
        m.font_ui,
        p.text,
    );
    kit::text(
        ctx,
        "changes apply immediately",
        layout.panel.left() + 16.0,
        layout.panel.top() + 48.0,
        m.font_small,
        p.faint,
    );

    for (index, row) in layout.rows.iter().enumerate() {
        let rect = layout.rows_rect[index];
        let selected = index == state.settings_row;
        if selected {
            kit::fill(ctx, rect, p.hover);
            kit::vline(ctx, rect.left(), rect.top(), rect.bottom(), p.accent);
        }
        let baseline = kit::baseline(measurer, rect, m.font_ui);
        kit::text(
            ctx,
            row.label(),
            rect.left() + 12.0,
            baseline,
            m.font_ui,
            if selected { p.text } else { p.dim },
        );

        if let (Some(minus), Some(plus)) = (layout.minus[index], layout.plus[index]) {
            paint_step(ctx, &theme, minus, "-");
            paint_step(ctx, &theme, plus, "+");
            let value = state.settings_value(*row);
            kit::text_right(
                ctx,
                measurer,
                &value,
                minus.left() - 10.0,
                baseline,
                m.font_ui,
                if selected { p.text } else { p.dim },
            );
        } else if *row == Row::Reset {
            kit::text_right(
                ctx,
                measurer,
                "RESET",
                rect.right() - 12.0,
                baseline,
                m.font_ui,
                p.error,
            );
        } else {
            let on = match row {
                Row::LineNumbers => state.settings.line_numbers,
                Row::IndentGuides => state.settings.indent_guides,
                Row::Syntax => state.settings.syntax,
                Row::LanguageIntelligence => state.settings.lsp,
                Row::TrustLocalServers => state.settings.trust_local_servers,
                Row::ReloadLsp => state.settings.lsp_reload,
                _ => false,
            };
            let label = if on { "ON" } else { "OFF" };
            kit::text_right(
                ctx,
                measurer,
                label,
                rect.right() - 12.0,
                baseline,
                m.font_ui,
                if on { p.accent } else { p.faint },
            );
        }
    }

    kit::text(
        ctx,
        "↑↓ select   ←→ change   Enter toggle   Esc close",
        layout.panel.left() + 16.0,
        layout.panel.bottom() - 14.0,
        m.font_small,
        p.faint,
    );
}

fn paint_step(ctx: &mut PaintContext, theme: &crate::theme::Theme, rect: Rect, label: &str) {
    let p = theme.p;
    kit::fill(ctx, rect, p.panel);
    ctx.stroke_rect(rect, 1.0, p.border);
    kit::text(
        ctx,
        label,
        rect.center().x - 3.0,
        rect.center().y + 5.0,
        theme.m.font_ui,
        p.text,
    );
}

/// Handles a pointer press. Returns `true` when the event is consumed.
pub fn click(state: &mut AppState, pos: Vec2) -> bool {
    if !state.settings_open {
        return false;
    }
    let layout = layout(state.viewport.logical_rect());
    if !layout.panel.contains(pos) {
        state.close_settings();
        return true;
    }
    for (index, rect) in layout.rows_rect.iter().enumerate() {
        if !rect.contains(pos) {
            continue;
        }
        state.settings_row = index;
        let row = layout.rows[index];
        if let Some(minus) = layout.minus[index] {
            if minus.contains(pos) {
                state.adjust_setting(row, -1);
                return true;
            }
        }
        if let Some(plus) = layout.plus[index] {
            if plus.contains(pos) {
                state.adjust_setting(row, 1);
                return true;
            }
        }
        state.activate_setting(row);
        return true;
    }
    true
}

/// Handles a key while the panel is open. Returns `true` when consumed.
pub fn on_key(state: &mut AppState, key: Key, modifiers: Modifiers) -> bool {
    if !state.settings_open {
        return false;
    }
    let accel = modifiers.meta || modifiers.ctrl;
    if accel {
        if matches!(key, Key::Character(',')) {
            state.close_settings();
            return true;
        }
        // App commands (save, close tab, …) still work with the panel open.
        return false;
    }
    match key {
        Key::Escape => state.close_settings(),
        Key::ArrowUp => {
            state.settings_row = state.settings_row.saturating_sub(1);
            state.dirty = true;
        }
        Key::ArrowDown => {
            state.settings_row = (state.settings_row + 1).min(ROWS.len() - 1);
            state.dirty = true;
        }
        Key::ArrowLeft => {
            let row = ROWS[state.settings_row.min(ROWS.len() - 1)];
            state.adjust_setting(row, -1);
        }
        Key::ArrowRight => {
            let row = ROWS[state.settings_row.min(ROWS.len() - 1)];
            state.adjust_setting(row, 1);
        }
        Key::Enter | Key::Space => {
            let row = ROWS[state.settings_row.min(ROWS.len() - 1)];
            state.activate_setting(row);
        }
        _ => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::session::Session;
    use crate::model::Workspace;
    use crate::theme::{Mode, Theme};

    fn state() -> AppState {
        let mut ws = Workspace::new(std::env::temp_dir());
        ws.new_tab();
        AppState::new(
            Theme::new(Mode::Dark),
            ws,
            Session::default(),
            std::env::temp_dir().join("igui-editor-settings-test.conf"),
        )
    }

    #[test]
    fn layout_rows_are_inside_the_panel() {
        let viewport = Rect::from_min_size(Vec2::ZERO, Size::new(1200.0, 800.0));
        let layout = layout(viewport);
        assert_eq!(layout.rows.len(), ROWS.len());
        for rect in &layout.rows_rect {
            assert!(
                layout.panel.contains_rect(*rect),
                "{rect:?} escapes the panel"
            );
        }
    }

    #[test]
    fn reload_lsp_row_toggles_and_persists_to_the_session() {
        let mut state = state();
        state.open_settings();
        state.adjust_setting(Row::ReloadLsp, 1);
        assert!(state.settings.lsp_reload);
        assert!(state.session.lsp_reload);
        assert!(state.lsp.aggressive_reload_enabled());
        state.adjust_setting(Row::ReloadLsp, 1);
        assert!(!state.settings.lsp_reload);
        assert!(!state.session.lsp_reload);
        assert!(!state.lsp.aggressive_reload_enabled());
    }

    #[test]
    fn stepping_font_and_tab_persists_to_the_session() {
        let mut state = state();
        state.open_settings();
        state.adjust_setting(Row::FontSize, 1);
        assert_eq!(state.settings.font_size, 14.0);
        state.adjust_setting(Row::TabWidth, 1);
        assert_eq!(state.settings.tab_width, 8);
        assert_eq!(state.session.font, 14.0);
        assert_eq!(state.session.tab, 8);
        assert!(state.config_dirty);
    }

    #[test]
    fn clicking_the_step_boxes_adjusts_the_row() {
        let mut state = state();
        state.viewport = igui_core::ViewportSize::new(igui_core::Size::new(1200.0, 800.0));
        state.open_settings();
        let layout = layout(state.viewport.logical_rect());
        let row = ROWS
            .iter()
            .position(|row| *row == Row::FontSize)
            .expect("font row");
        let plus = layout.plus[row].expect("font row has a + box");
        assert!(click(&mut state, plus.center()));
        assert_eq!(state.settings.font_size, 14.0);
        let minus = layout.minus[row].expect("font row has a - box");
        assert!(click(&mut state, minus.center()));
        assert_eq!(state.settings.font_size, 13.0);
        let toggle_row = ROWS
            .iter()
            .position(|row| *row == Row::LineNumbers)
            .expect("line-number row");
        assert!(click(&mut state, layout.rows_rect[toggle_row].center()));
        assert!(!state.settings.line_numbers);
    }

    #[test]
    fn clicking_outside_closes_and_reset_restores_defaults() {
        let mut state = state();
        state.viewport = igui_core::ViewportSize::new(igui_core::Size::new(1200.0, 800.0));
        state.open_settings();
        state.settings.font_size = 18.0;
        state.activate_setting(Row::Reset);
        assert_eq!(state.settings.font_size, 13.0);
        let outside = Vec2::new(2.0, 2.0);
        assert!(click(&mut state, outside));
        assert!(!state.settings_open);
    }
}
