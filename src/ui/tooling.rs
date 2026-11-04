//! Missing-tool prompt: copy the install command, never run it.
//!
//! One `layout()` derives the panel and every hit rectangle, so paint and
//! pointer handling cannot drift apart (same rule as the settings panel).

use igui_core::{Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit::{self, RectExt};
use crate::state::AppState;

const PANEL_W: f32 = 580.0;
const PAD: f32 = 18.0;
const TITLE_H: f32 = 56.0;
const BUTTON_H: f32 = 30.0;
const BUTTON_W: f32 = 128.0;
const FOOTER_H: f32 = 34.0;

pub struct Layout {
    pub panel: Rect,
    pub title: Rect,
    pub detail: f32,
    pub command: Rect,
    pub note: f32,
    pub copy: Option<Rect>,
    pub retry: Rect,
    pub close: Rect,
}

pub fn layout(state: &AppState) -> Layout {
    let viewport = state.viewport.logical_rect();
    let width = PANEL_W.min((viewport.width() - 40.0).max(320.0));
    let command = state
        .tooling
        .prompt
        .as_ref()
        .map(|prompt| !prompt.command.is_empty())
        .unwrap_or(false);
    let height = TITLE_H + 26.0 + 20.0 + 40.0 + 22.0 + 52.0 + FOOTER_H;
    let panel = Rect::from_center_size(viewport.center(), Size::new(width, height));
    let left = panel.left() + PAD;
    let inner = (panel.width() - PAD * 2.0).max(0.0);
    let title = Rect::from_min_size(
        Vec2::new(left, panel.top() + 14.0),
        Size::new(inner, TITLE_H),
    );
    let detail = panel.top() + TITLE_H + 20.0;
    let command_rect = Rect::from_min_size(Vec2::new(left, detail + 18.0), Size::new(inner, 40.0));
    let note = command_rect.bottom() + 22.0;
    let buttons_y = panel.bottom() - FOOTER_H - BUTTON_H - 8.0;
    let close = Rect::from_min_size(
        Vec2::new(panel.right() - PAD - 88.0, buttons_y),
        Size::new(88.0, BUTTON_H),
    );
    let retry = Rect::from_min_size(
        Vec2::new(close.left() - 8.0 - 88.0, buttons_y),
        Size::new(88.0, BUTTON_H),
    );
    let copy = command.then(|| {
        Rect::from_min_size(
            Vec2::new(retry.left() - 8.0 - BUTTON_W, buttons_y),
            Size::new(BUTTON_W, BUTTON_H),
        )
    });
    Layout {
        panel,
        title,
        detail,
        command: command_rect,
        note,
        copy,
        retry,
        close,
    }
}

pub fn paint(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    if state.tooling.prompt.is_none() {
        return;
    }
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let layout = layout(state);
    let viewport = state.viewport.logical_rect();
    ctx.fill_rect(viewport, igui_core::Color::new(0.0, 0.0, 0.0, 0.35));
    kit::fill(ctx, layout.panel, p.raised);
    ctx.stroke_rect(layout.panel, 1.0, p.border);
    kit::hline(
        ctx,
        layout.panel.left(),
        layout.panel.right(),
        layout.panel.top(),
        p.accent,
    );

    let prompt = state.tooling.prompt.clone().unwrap_or_default();
    let title = format!("{} NOT FOUND", prompt.tool.to_uppercase());
    kit::text(
        ctx,
        &kit::truncate(measurer, &title, m.font_ui, layout.title.width()),
        layout.title.left(),
        layout.title.top() + 26.0,
        m.font_ui,
        p.text,
    );
    let subtitle = format!(
        "{} language features are disabled until it is installed.",
        prompt.language
    );
    kit::text(
        ctx,
        &kit::truncate(measurer, &subtitle, m.font_small, layout.title.width()),
        layout.title.left(),
        layout.title.top() + 46.0,
        m.font_small,
        p.faint,
    );

    let detail = kit::truncate(measurer, &prompt.detail, m.font_small, layout.title.width());
    kit::text(
        ctx,
        &detail,
        layout.title.left(),
        layout.detail,
        m.font_small,
        p.dim,
    );

    // Command box: the exact line to paste in a terminal.
    kit::fill(ctx, layout.command, p.panel);
    ctx.stroke_rect(layout.command, 1.0, p.border);
    let text = if prompt.command.is_empty() {
        "no install command for this server".to_string()
    } else {
        prompt.command.clone()
    };
    let inner = layout.command.left() + 10.0;
    let available = (layout.command.width() - 20.0).max(24.0);
    let shown = crate::kit::truncate_left(measurer, &text, m.font_small, available);
    kit::text(
        ctx,
        &shown,
        inner,
        kit::baseline(measurer, layout.command, m.font_small),
        m.font_small,
        if prompt.command.is_empty() {
            p.faint
        } else {
            p.text
        },
    );

    if let Some(note) = prompt.note.as_deref() {
        let note = kit::truncate(measurer, note, m.font_small, layout.title.width());
        kit::text(
            ctx,
            &note,
            layout.title.left(),
            layout.note,
            m.font_small,
            p.faint,
        );
    }

    if let Some(copy) = layout.copy {
        let label = if prompt.copied {
            "COPIED"
        } else {
            "COPY COMMAND"
        };
        paint_button(ctx, measurer, &theme, copy, label, true, prompt.copied);
    }
    paint_button(ctx, measurer, &theme, layout.retry, "RETRY", false, false);
    paint_button(ctx, measurer, &theme, layout.close, "CLOSE", false, false);

    kit::text(
        ctx,
        "No automatic installs · run the command in a terminal, then retry",
        layout.panel.left() + PAD,
        layout.panel.bottom() - 12.0,
        m.font_small,
        p.faint,
    );
}

fn paint_button(
    ctx: &mut PaintContext,
    measurer: &dyn TextMeasurer,
    theme: &crate::theme::Theme,
    rect: Rect,
    label: &str,
    primary: bool,
    confirmed: bool,
) {
    let p = theme.p;
    let fill = if confirmed {
        p.accent
    } else if primary {
        p.hover
    } else {
        p.panel
    };
    kit::fill(ctx, rect, fill);
    ctx.stroke_rect(rect, 1.0, if primary { p.accent } else { p.border });
    kit::text_center(
        ctx,
        measurer,
        label,
        rect,
        kit::baseline(measurer, rect, theme.m.font_small),
        theme.m.font_small,
        if primary { p.accent } else { p.dim },
    );
}

/// Pointer press. While a prompt is open it swallows every click so the editor
/// behind it cannot change under the user.
pub fn click(state: &mut AppState, pos: Vec2) -> bool {
    if state.tooling.prompt.is_none() {
        return false;
    }
    let layout = layout(state);
    if layout.close.contains(pos) {
        state.close_tool_prompt();
        return true;
    }
    if layout.retry.contains(pos) {
        state.retry_servers();
        return true;
    }
    if layout.copy.is_some_and(|copy| copy.contains(pos)) {
        state.copy_tool_command();
        return true;
    }
    true
}
