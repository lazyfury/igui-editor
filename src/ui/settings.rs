//! The settings panel: a centered overlay with a search field, a left group
//! navigation and one scrolling pane of rows (one widget per [`RowKind`]).
//!
//! Every row applies immediately and is persisted through the session by
//! `AppState`; the panel only paints values and dispatches adjustments.
//!
//! One [`layout`] function serves paint, hit testing, scrolling and keyboard
//! reveal. Row order, groups and widget kinds all come from
//! [`crate::settings::ROWS`]; this module never lists a row itself.
//!
//! The pane shows the selected row's group (or every search match, with group
//! headers). The body height is capped, so a tall window does not stretch the
//! panel. Keys: typing filters the list, `↑`/`↓` walk rows across groups,
//! `Tab`/`Shift+Tab` switch group, `←`/`→` adjust (or move the search caret when
//! the query is non-empty), `Enter` toggles / rotates / starts typing a number,
//! `Esc` clears the query and then closes. The pointer can press a group, a
//! switch, choice buttons/arrows and the `−`/`+` boxes.

use igui_core::{Color, Key, Modifiers, Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::{TextEdit, TextMeasurer};

use crate::kit::{self, RectExt};
use crate::settings::{Group, Row, RowKind, RowSpec};
use crate::state::AppState;

const PANEL_W: f32 = 720.0;
const ROW_H: f32 = 30.0;
/// Vertical gap above and below each row/nav item (the pitch stays `ROW_H`).
const ROW_GAP: f32 = 1.0;
const GROUP_H: f32 = 26.0;
/// Panel edge padding and the one spacing unit every gap is a multiple of.
const PANEL_PAD: f32 = 16.0;
const GAP: f32 = 8.0;
/// Header: title and search share one row.
const FIELD_H: f32 = 30.0;
const HEADER_TOP: f32 = 12.0;
const TITLE_W: f32 = 76.0;
const TITLE_GAP: f32 = 12.0;
const HEADER_H: f32 = HEADER_TOP + FIELD_H + 14.0;
const FOOTER_H: f32 = 44.0;
const BODY_PAD: f32 = GAP;
const SWITCH_W: f32 = 36.0;
const SWITCH_H: f32 = 18.0;
const SWITCH_GAP: f32 = GAP;
const STEP_W: f32 = 26.0;
const STEP_GAP: f32 = GAP;
const VALUE_W: f32 = 76.0;
const CHOICE_BUTTON_W: f32 = 76.0;
const CHOICE_BUTTON_GAP: f32 = 4.0;
const ARROW_W: f32 = 22.0;
const ARROW_GAP: f32 = GAP;
const RESET_W: f32 = 140.0;
const RESET_H: f32 = 28.0;
/// Height ceiling of the scrolling right pane; keeps the panel compact on tall
/// windows instead of stretching to the viewport.
const MAX_BODY_H: f32 = 336.0;
const NAV_W: f32 = 168.0;
const NAV_ITEM_H: f32 = ROW_H;
const NAV_PAD: f32 = GAP;
/// Navigation needs `GROUPS` * item height, even when the pane is short.
const NAV_MIN_H: f32 = NAV_PAD * 2.0 + NAV_ITEM_H * 5.0;
/// Choice rows with fewer options than this paint as a `[ A | B ]` button
/// group; the rest paint as `< value >` arrows.
const CHOICE_BUTTONS_MAX: usize = 3;

/// One selectable row and its recorded widget rectangles.
pub struct RowSlot {
    pub spec: &'static RowSpec,
    pub rect: Rect,
    /// Button-group options, `(index, rect)`; empty for other kinds.
    pub options: Vec<(usize, Rect)>,
    /// `−`/left-arrow box and `+`/right-arrow box.
    pub minus: Option<Rect>,
    pub plus: Option<Rect>,
    /// Value area (a typed field while editing).
    pub value: Rect,
    /// Toggle switch box.
    pub switch_box: Rect,
}

impl RowSlot {
    /// Left edge of the row's widgets, so the label can be truncated before it
    /// runs underneath them.
    fn controls_left(&self) -> f32 {
        if let Some((_, first)) = self.options.first() {
            return first.left();
        }
        if let Some(minus) = self.minus {
            return minus.left();
        }
        if self.switch_box.size.width > 0.0 {
            return self.switch_box.left() - 40.0;
        }
        self.rect.right()
    }
}

pub struct GroupSlot {
    pub group: Group,
    pub rect: Rect,
}

/// One left-navigation entry.
pub struct NavSlot {
    pub group: Group,
    pub rect: Rect,
    /// The group has at least one row under the current search query.
    pub enabled: bool,
}

/// Panel, navigation, scrolling content and scroll limits. One function for
/// paint, hit tests and reveal.
pub struct Layout {
    pub panel: Rect,
    pub title: Rect,
    pub search: Rect,
    pub nav: Rect,
    pub content: Rect,
    pub nav_items: Vec<NavSlot>,
    /// Group headers inside the content pane (search results only).
    pub groups: Vec<GroupSlot>,
    pub rows: Vec<RowSlot>,
    pub footer: Rect,
    pub reset: Rect,
    pub max_scroll: f32,
    pub scroll: f32,
}

pub fn layout(state: &AppState) -> Layout {
    let viewport = state.viewport.logical_rect();
    let pane = state.settings_pane();
    let searching = !state.settings_ui.query.text().is_empty();

    let mut content_h = BODY_PAD * 2.0;
    let mut last_group: Option<Group> = None;
    for &spec in &pane {
        if searching && last_group != Some(spec.group) {
            content_h += GROUP_H;
            last_group = Some(spec.group);
        }
        content_h += ROW_H;
    }

    // The panel never grows past the window: the body takes the content height
    // (at least the navigation's), capped by `MAX_BODY_H` and by what the window
    // leaves between the 20 px margins.
    let available = (viewport.height() - 40.0 - HEADER_H - FOOTER_H).max(0.0);
    let max_body = available.min(MAX_BODY_H);
    let body_h = content_h.max(NAV_MIN_H).min(max_body);
    let width = PANEL_W.min((viewport.width() - 40.0).max(320.0));
    let height = HEADER_H + body_h + FOOTER_H;
    let panel = Rect::from_center_size(viewport.center(), Size::new(width, height));

    let title = Rect::from_min_size(
        Vec2::new(panel.left() + PANEL_PAD, panel.top() + HEADER_TOP),
        Size::new(TITLE_W, FIELD_H),
    );
    let search = Rect::from_min_size(
        Vec2::new(title.right() + TITLE_GAP, title.top()),
        Size::new(
            (panel.right() - PANEL_PAD - title.right() - TITLE_GAP).max(80.0),
            FIELD_H,
        ),
    );
    let body = Rect::from_min_size(
        Vec2::new(panel.left(), panel.top() + HEADER_H),
        Size::new(panel.width(), body_h),
    );
    let nav = Rect::from_min_size(
        body.origin,
        Size::new(NAV_W.min(body.width() - 240.0), body_h),
    );
    let content = Rect::from_min_size(
        Vec2::new(nav.right(), body.top()),
        Size::new(body.width() - nav.width(), body_h),
    );
    let footer = Rect::from_min_size(
        Vec2::new(panel.left(), panel.bottom() - FOOTER_H),
        Size::new(panel.width(), FOOTER_H),
    );
    let max_scroll = (content_h - body_h).max(0.0);
    let scroll = state.settings_ui.scroll.clamp(0.0, max_scroll);

    let visible = state.settings_visible();
    let mut nav_items = Vec::new();
    let mut y = nav.top() + NAV_PAD;
    for group in AppState::settings_groups() {
        nav_items.push(NavSlot {
            group,
            rect: Rect::from_min_size(
                Vec2::new(nav.left() + 8.0, y + ROW_GAP),
                Size::new((nav.width() - 16.0).max(40.0), NAV_ITEM_H - ROW_GAP * 2.0),
            ),
            enabled: visible.iter().any(|spec| spec.group == group),
        });
        y += NAV_ITEM_H;
    }

    let mut groups = Vec::new();
    let mut rows = Vec::with_capacity(pane.len());
    let mut y = content.top() + BODY_PAD - scroll;
    let mut last_group: Option<Group> = None;
    for &spec in &pane {
        if searching && last_group != Some(spec.group) {
            groups.push(GroupSlot {
                group: spec.group,
                rect: Rect::from_min_size(
                    Vec2::new(content.left() + PANEL_PAD, y),
                    Size::new(content.width() - PANEL_PAD * 2.0, GROUP_H),
                ),
            });
            y += GROUP_H;
            last_group = Some(spec.group);
        }
        let rect = Rect::from_min_size(
            Vec2::new(content.left() + 8.0, y + ROW_GAP),
            Size::new(content.width() - 16.0, ROW_H - ROW_GAP * 2.0),
        );
        rows.push(row_slot(spec, rect));
        y += ROW_H;
    }

    let reset = Rect::from_min_size(
        Vec2::new(
            footer.right() - PANEL_PAD - RESET_W,
            footer.center().y - RESET_H * 0.5,
        ),
        Size::new(RESET_W, RESET_H),
    );

    Layout {
        panel,
        title,
        search,
        nav,
        content,
        nav_items,
        groups,
        rows,
        footer,
        reset,
        max_scroll,
        scroll,
    }
}

fn row_slot(spec: &'static RowSpec, rect: Rect) -> RowSlot {
    let baseline_widget = |width: f32| {
        Rect::from_min_size(
            Vec2::new(rect.right() - 12.0 - width, rect.top()),
            Size::new(width, rect.height()),
        )
    };
    let mut options = Vec::new();
    let mut minus = None;
    let mut plus = None;
    let mut switch_box = Rect::ZERO;
    let value;
    match spec.kind {
        RowKind::Toggle => {
            switch_box = Rect::from_min_size(
                Vec2::new(
                    rect.right() - 12.0 - SWITCH_W,
                    rect.center().y - SWITCH_H * 0.5,
                ),
                Size::new(SWITCH_W, SWITCH_H),
            );
            value = Rect::ZERO;
        }
        RowKind::Choice(options_) if options_.len() < CHOICE_BUTTONS_MAX => {
            let width = options_.len() as f32 * CHOICE_BUTTON_W
                + (options_.len().saturating_sub(1)) as f32 * CHOICE_BUTTON_GAP;
            let group = baseline_widget(width);
            let mut x = group.left();
            for index in 0..options_.len() {
                options.push((
                    index,
                    Rect::from_min_size(
                        Vec2::new(x, group.top() + 3.0),
                        Size::new(CHOICE_BUTTON_W, group.height() - 6.0),
                    ),
                ));
                x += CHOICE_BUTTON_W + CHOICE_BUTTON_GAP;
            }
            value = Rect::ZERO;
        }
        RowKind::Choice(_) => {
            let group = baseline_widget(ARROW_W * 2.0 + ARROW_GAP * 2.0 + VALUE_W);
            minus = Some(Rect::from_min_size(
                Vec2::new(group.left(), group.top() + 3.0),
                Size::new(ARROW_W, group.height() - 6.0),
            ));
            plus = Some(Rect::from_min_size(
                Vec2::new(group.right() - ARROW_W, group.top() + 3.0),
                Size::new(ARROW_W, group.height() - 6.0),
            ));
            value = Rect::from_min_size(
                Vec2::new(group.left() + ARROW_W + ARROW_GAP, group.top() + 3.0),
                Size::new(VALUE_W, group.height() - 6.0),
            );
        }
        RowKind::Number(_) => {
            let group = baseline_widget(STEP_W * 2.0 + STEP_GAP * 2.0 + VALUE_W);
            minus = Some(Rect::from_min_size(
                Vec2::new(group.left(), group.top() + 2.0),
                Size::new(STEP_W, group.height() - 4.0),
            ));
            plus = Some(Rect::from_min_size(
                Vec2::new(group.right() - STEP_W, group.top() + 2.0),
                Size::new(STEP_W, group.height() - 4.0),
            ));
            value = Rect::from_min_size(
                Vec2::new(group.left() + STEP_W + STEP_GAP, group.top() + 2.0),
                Size::new(VALUE_W, group.height() - 4.0),
            );
        }
        RowKind::Action => {
            value = Rect::ZERO;
        }
    }
    RowSlot {
        spec,
        rect,
        options,
        minus,
        plus,
        value,
        switch_box,
    }
}

/// The number being typed, if any: its row and current field text.
fn editing(state: &AppState) -> Option<(Row, &TextEdit)> {
    state
        .settings_ui
        .editing
        .as_ref()
        .map(|edit| (edit.row, &edit.field))
}

pub fn paint(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    if !state.settings_open {
        return;
    }
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let viewport = state.viewport.logical_rect();
    let layout = layout(state);
    // Keep the stored offset inside the current window after a resize/filter.
    state.settings_ui.scroll = layout.scroll;
    let selected = state.settings_ui.row;
    let editing_row = editing(state).map(|(row, _)| row);

    ctx.fill_rect(viewport, Color::new(0.0, 0.0, 0.0, 0.4));
    kit::fill(ctx, layout.panel, p.raised);
    ctx.stroke_rect(layout.panel, 1.0, p.border);
    kit::hline(
        ctx,
        layout.panel.left(),
        layout.panel.right(),
        layout.panel.top(),
        p.accent,
    );

    // Title and search share one row: the search is the active input unless a
    // number is being typed.
    kit::text(
        ctx,
        "Settings",
        layout.title.left(),
        kit::baseline(measurer, layout.title, m.font_ui),
        m.font_ui,
        p.text,
    );
    let search_edit = &state.settings_ui.query;
    let query_empty = search_edit.text().is_empty() && state.settings_ui.preedit.is_none();
    let search_caret = paint_field(
        ctx,
        measurer,
        &theme,
        layout.search,
        search_edit,
        if editing_row.is_some() {
            None
        } else {
            state.settings_ui.preedit.as_deref()
        },
        FieldStyle {
            border: if editing_row.is_some() {
                p.border
            } else {
                p.accent
            },
            placeholder: if query_empty {
                Some("Search settings…")
            } else {
                None
            },
        },
    );
    state.settings_ui.search_caret = search_caret;

    // Left navigation: one group per row, the active group highlighted.
    let active_group = state.settings_group();
    kit::fill(ctx, layout.nav, p.panel);
    kit::vline(
        ctx,
        layout.nav.right(),
        layout.nav.top(),
        layout.nav.bottom(),
        p.border,
    );
    for item in &layout.nav_items {
        let active = item.group == active_group;
        if active {
            kit::fill(ctx, item.rect, p.hover);
            kit::vline(
                ctx,
                item.rect.left(),
                item.rect.top(),
                item.rect.bottom(),
                p.accent,
            );
        }
        kit::text(
            ctx,
            item.group.label(),
            item.rect.left() + 10.0,
            kit::baseline(measurer, item.rect, m.font_ui),
            m.font_ui,
            if !item.enabled {
                p.faint
            } else if active {
                p.text
            } else {
                p.dim
            },
        );
    }

    ctx.save();
    ctx.clip_rect(layout.content);
    for group in &layout.groups {
        kit::text(
            ctx,
            group.group.label(),
            group.rect.left() + 8.0,
            group.rect.bottom() - 8.0,
            m.font_small,
            p.faint,
        );
        kit::hline(
            ctx,
            group.rect.left() + 8.0,
            group.rect.right(),
            group.rect.bottom() - 2.0,
            p.border,
        );
    }
    let mut edit_caret = Rect::ZERO;
    for slot in &layout.rows {
        let rect = slot.rect;
        let is_selected = slot.spec.row == selected;
        if is_selected {
            kit::fill(ctx, rect, p.hover);
            kit::vline(ctx, rect.left(), rect.top(), rect.bottom(), p.accent);
        }
        let baseline = kit::baseline(measurer, rect, m.font_ui);
        let label_room = (slot.controls_left() - rect.left() - 20.0).max(40.0);
        let label = kit::truncate(measurer, slot.spec.row.label(), m.font_ui, label_room);
        kit::text(
            ctx,
            &label,
            rect.left() + 12.0,
            baseline,
            m.font_ui,
            if is_selected { p.text } else { p.dim },
        );

        match slot.spec.kind {
            RowKind::Toggle => {
                let on = state.settings_toggle_state(slot.spec.row);
                paint_switch(ctx, &theme, slot.switch_box, on);
                kit::text_right(
                    ctx,
                    measurer,
                    if on { "ON" } else { "OFF" },
                    slot.switch_box.left() - SWITCH_GAP,
                    baseline,
                    m.font_small,
                    if on { p.accent } else { p.faint },
                );
            }
            RowKind::Choice(_) if !slot.options.is_empty() => {
                let index = state.settings_choice_index(slot.spec.row).unwrap_or(0);
                let options = choice_options(slot.spec);
                for (option, option_rect) in &slot.options {
                    let on = *option == index;
                    kit::fill(ctx, *option_rect, if on { p.selection } else { p.panel });
                    ctx.stroke_rect(*option_rect, 1.0, if on { p.accent } else { p.border });
                    kit::text_center(
                        ctx,
                        measurer,
                        options.get(*option).copied().unwrap_or(""),
                        *option_rect,
                        kit::baseline(measurer, *option_rect, m.font_small),
                        m.font_small,
                        if on { p.accent } else { p.dim },
                    );
                }
            }
            RowKind::Choice(_) => {
                let left = slot.minus.unwrap_or(Rect::ZERO);
                let right = slot.plus.unwrap_or(Rect::ZERO);
                paint_arrow(ctx, &theme, left, true, p.dim);
                paint_arrow(ctx, &theme, right, false, p.dim);
                kit::text_center(
                    ctx,
                    measurer,
                    &state.settings_value(slot.spec.row),
                    slot.value,
                    baseline,
                    m.font_ui,
                    if is_selected { p.text } else { p.dim },
                );
            }
            RowKind::Number(_) => {
                paint_step(ctx, &theme, slot.minus.unwrap_or(Rect::ZERO), "-");
                paint_step(ctx, &theme, slot.plus.unwrap_or(Rect::ZERO), "+");
                if editing_row == Some(slot.spec.row) {
                    if let Some((_, field)) = editing(state) {
                        edit_caret = paint_field(
                            ctx,
                            measurer,
                            &theme,
                            slot.value,
                            field,
                            state.settings_ui.preedit.as_deref(),
                            FieldStyle {
                                border: p.accent,
                                placeholder: None,
                            },
                        );
                    }
                } else {
                    paint_value(ctx, measurer, &theme, slot, state, baseline);
                }
            }
            RowKind::Action => {}
        }
    }
    if layout.rows.is_empty() {
        kit::text(
            ctx,
            "No settings match",
            layout.content.left() + PANEL_PAD + 8.0,
            layout.content.top() + 26.0,
            m.font_small,
            p.faint,
        );
    }
    ctx.restore();
    state.settings_ui.edit_caret = edit_caret;

    kit::hline(
        ctx,
        layout.footer.left(),
        layout.footer.right(),
        layout.footer.top(),
        p.border,
    );
    kit::text(
        ctx,
        "type to search · ↑↓ row · Tab group · ←→ change · Enter edit · Esc close",
        layout.footer.left() + PANEL_PAD,
        layout.footer.center().y + 4.0,
        m.font_small,
        p.faint,
    );
    ctx.stroke_rect(layout.reset, 1.0, p.border);
    kit::text_center(
        ctx,
        measurer,
        "RESET TO DEFAULTS",
        layout.reset,
        kit::baseline(measurer, layout.reset, m.font_small),
        m.font_small,
        p.error,
    );
}

fn choice_options(spec: &'static RowSpec) -> &'static [&'static str] {
    match spec.kind {
        RowKind::Choice(options) => options,
        _ => &[],
    }
}

fn paint_value(
    ctx: &mut PaintContext,
    measurer: &dyn TextMeasurer,
    theme: &crate::theme::Theme,
    slot: &RowSlot,
    state: &AppState,
    baseline: f32,
) {
    let p = theme.p;
    let m = theme.m;
    let rect = slot.value;
    let row = slot.spec.row;
    let RowKind::Number(spec) = slot.spec.kind else {
        return;
    };
    let value = state.settings_value(row);
    let text = value
        .strip_suffix(spec.unit)
        .map(str::trim_end)
        .unwrap_or(&value);
    kit::text_center(
        ctx,
        measurer,
        text,
        rect,
        baseline,
        m.font_ui,
        if row == state.settings_ui.row {
            p.text
        } else {
            p.dim
        },
    );
    if !spec.unit.is_empty() {
        kit::text(
            ctx,
            spec.unit,
            rect.right() - kit::width(measurer, spec.unit, m.font_small),
            baseline,
            m.font_small,
            p.faint,
        );
    }
}

fn paint_switch(ctx: &mut PaintContext, theme: &crate::theme::Theme, rect: Rect, on: bool) {
    let p = theme.p;
    kit::fill(ctx, rect, if on { p.selection } else { p.panel });
    ctx.stroke_rect(rect, 1.0, if on { p.accent } else { p.border });
    let knob_w = rect.width() * 0.5 - 3.0;
    let x = if on {
        rect.right() - 3.0 - knob_w
    } else {
        rect.left() + 3.0
    };
    kit::fill(
        ctx,
        Rect::from_min_size(
            Vec2::new(x, rect.top() + 3.0),
            Size::new(knob_w, rect.height() - 6.0),
        ),
        if on { p.accent } else { p.faint },
    );
}

fn paint_step(ctx: &mut PaintContext, theme: &crate::theme::Theme, rect: Rect, label: &str) {
    let p = theme.p;
    let m = theme.m;
    kit::fill(ctx, rect, p.panel);
    ctx.stroke_rect(rect, 1.0, p.border);
    kit::text(
        ctx,
        label,
        rect.center().x - 3.0,
        rect.center().y + 5.0,
        m.font_ui,
        p.text,
    );
}

fn paint_arrow(
    ctx: &mut PaintContext,
    theme: &crate::theme::Theme,
    rect: Rect,
    left: bool,
    color: Color,
) {
    let m = theme.m;
    let glyph = if left { "<" } else { ">" };
    let width = m.font_ui * 0.5;
    kit::text(
        ctx,
        glyph,
        rect.center().x - width * 0.5,
        rect.center().y + 5.0,
        m.font_ui,
        color,
    );
}

/// Border color and placeholder of a painted field.
struct FieldStyle<'a> {
    border: Color,
    placeholder: Option<&'a str>,
}

/// Paints a single-line text field and returns its caret rectangle (viewport
/// space), for the host's IME placement.
fn paint_field(
    ctx: &mut PaintContext,
    measurer: &dyn TextMeasurer,
    theme: &crate::theme::Theme,
    rect: Rect,
    edit: &TextEdit,
    preedit: Option<&str>,
    style: FieldStyle<'_>,
) -> Rect {
    let p = theme.p;
    let m = theme.m;
    kit::fill(ctx, rect, p.bg);
    ctx.stroke_rect(rect, 1.0, style.border);
    let inner = Rect::from_min_size(
        Vec2::new(rect.left() + 8.0, rect.top()),
        Size::new((rect.width() - 16.0).max(8.0), rect.height()),
    );
    ctx.save();
    ctx.clip_rect(inner);
    let baseline = kit::baseline(measurer, rect, m.font_ui);
    if let Some(placeholder) = style.placeholder {
        kit::text(ctx, placeholder, inner.left(), baseline, m.font_ui, p.faint);
    }
    if let Some((start, end)) = edit.selection() {
        let x0 = inner.left() + kit::width(measurer, &edit.text()[..start], m.font_ui);
        let x1 = inner.left() + kit::width(measurer, &edit.text()[..end], m.font_ui);
        ctx.fill_rect(
            Rect::from_min_size(
                Vec2::new(x0, rect.top() + 4.0),
                Size::new((x1 - x0).max(1.0), rect.height() - 8.0),
            ),
            p.selection,
        );
    }
    kit::text(ctx, edit.text(), inner.left(), baseline, m.font_ui, p.text);
    let mut caret_x = inner.left() + kit::width(measurer, &edit.text()[..edit.caret()], m.font_ui);
    if let Some(preedit) = preedit {
        kit::text(ctx, preedit, caret_x, baseline, m.font_ui, p.accent);
        caret_x += kit::width(measurer, preedit, m.font_ui);
    }
    let caret = kit::field_caret(inner, caret_x, rect);
    ctx.fill_rect(caret, p.accent);
    ctx.restore();
    caret
}

/// Scrolls the body under the pointer. Wheel deltas are logical pixels.
pub fn scroll(state: &mut AppState, pos: Vec2, delta_y: f32) {
    if !state.settings_open {
        return;
    }
    let layout = layout(state);
    if !layout.panel.contains(pos) {
        return;
    }
    let before = state.settings_ui.scroll;
    crate::ui::scroll::apply(&mut state.settings_ui.scroll, delta_y, layout.max_scroll);
    if state.settings_ui.scroll != before {
        state.dirty = true;
    }
}

/// Adjusts the scroll so the selected row is fully inside the body.
fn reveal_selected(state: &mut AppState) {
    let layout = layout(state);
    let Some(slot) = layout
        .rows
        .iter()
        .find(|slot| slot.spec.row == state.settings_ui.row)
    else {
        return;
    };
    let mut scroll = layout.scroll;
    if slot.rect.top() < layout.content.top() {
        scroll -= layout.content.top() - slot.rect.top();
    } else if slot.rect.bottom() > layout.content.bottom() {
        scroll += slot.rect.bottom() - layout.content.bottom();
    }
    let scroll = scroll.clamp(0.0, layout.max_scroll);
    if (scroll - state.settings_ui.scroll).abs() > 0.5 {
        state.settings_ui.scroll = scroll;
        state.dirty = true;
    }
}

/// Handles a pointer press. Returns `true` when the event is consumed.
pub fn click(state: &mut AppState, pos: Vec2) -> bool {
    if !state.settings_open {
        return false;
    }
    let layout = layout(state);
    if !layout.panel.contains(pos) {
        state.close_settings();
        return true;
    }
    if layout.search.contains(pos) {
        state.settings_cancel_edit();
        state.dirty = true;
        return true;
    }
    if layout.reset.contains(pos) {
        state.reset_settings();
        return true;
    }
    for item in &layout.nav_items {
        if item.rect.contains(pos) {
            if item.enabled {
                state.settings_show_group(item.group);
            }
            return true;
        }
    }
    for slot in &layout.rows {
        if !slot.rect.contains(pos) {
            continue;
        }
        let row = slot.spec.row;
        // Moving off a typed number applies it, like blurring a field.
        if state.settings_editing().is_some() {
            state.settings_commit_edit();
        }
        state.settings_ui.row = row;
        state.dirty = true;
        match slot.spec.kind {
            RowKind::Toggle => {
                state.toggle_setting(row);
            }
            RowKind::Choice(options) if !slot.options.is_empty() => {
                if let Some((index, _)) = slot.options.iter().find(|(_, rect)| rect.contains(pos)) {
                    state.settings_set_choice(row, *index);
                } else {
                    state.adjust_setting(row, if options.len() > 1 { 1 } else { 0 });
                }
            }
            RowKind::Choice(_) => {
                if let Some(left) = slot.minus {
                    if left.contains(pos) {
                        state.adjust_setting(row, -1);
                        return true;
                    }
                }
                if let Some(right) = slot.plus {
                    if right.contains(pos) {
                        state.adjust_setting(row, 1);
                        return true;
                    }
                }
                state.adjust_setting(row, 1);
            }
            RowKind::Number(_) => {
                if slot.minus.is_some_and(|minus| minus.contains(pos)) {
                    state.adjust_setting(row, -1);
                    return true;
                }
                if slot.plus.is_some_and(|plus| plus.contains(pos)) {
                    state.adjust_setting(row, 1);
                    return true;
                }
                state.settings_begin_edit(row);
            }
            RowKind::Action => state.reset_settings(),
        }
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
    let shift = modifiers.shift;
    if accel {
        if matches!(key, Key::Character(',')) {
            state.close_settings();
            return true;
        }
        if clipboard_key(state, key) {
            state.dirty = true;
            return true;
        }
        // App commands (save, close tab, …) still work with the panel open.
        return false;
    }

    if state.settings_editing().is_some() {
        match key {
            Key::Escape => {
                state.settings_cancel_edit();
                return true;
            }
            Key::Enter => {
                state.settings_commit_edit();
                return true;
            }
            _ => {}
        }
        if let Some(field) = state
            .settings_ui
            .editing
            .as_mut()
            .map(|edit| &mut edit.field)
        {
            match key {
                Key::Backspace => field.backspace(),
                Key::Delete => field.delete(),
                Key::ArrowLeft => field.move_left(shift, modifiers.alt),
                Key::ArrowRight => field.move_right(shift, modifiers.alt),
                Key::Home => field.move_home(shift),
                Key::End => field.move_end(shift),
                _ => {}
            }
        }
        state.dirty = true;
        return true;
    }

    let has_query = !state.settings_ui.query.text().is_empty();
    match key {
        Key::Escape => {
            if has_query {
                state.settings_ui.query.set_text("");
                state.settings_query_changed();
            } else {
                state.close_settings();
            }
        }
        Key::ArrowUp => {
            state.settings_select(-1);
            reveal_selected(state);
        }
        Key::ArrowDown => {
            state.settings_select(1);
            reveal_selected(state);
        }
        Key::ArrowLeft if has_query => state.settings_ui.query.move_left(shift, modifiers.alt),
        Key::ArrowRight if has_query => state.settings_ui.query.move_right(shift, modifiers.alt),
        Key::ArrowLeft => {
            state.adjust_setting(state.settings_ui.row, -1);
        }
        Key::ArrowRight => {
            state.adjust_setting(state.settings_ui.row, 1);
        }
        Key::Home if has_query => state.settings_ui.query.move_home(shift),
        Key::End if has_query => state.settings_ui.query.move_end(shift),
        Key::Enter => state.activate_setting(state.settings_ui.row),
        Key::Tab => {
            state.settings_select_group(if shift { -1 } else { 1 });
            reveal_selected(state);
        }
        // The host sends Space as a named key, not committed text.
        Key::Space => state.settings_text(" "),
        Key::Backspace if has_query => {
            state.settings_ui.query.backspace();
            state.settings_query_changed();
        }
        Key::Delete if has_query => {
            state.settings_ui.query.delete();
            state.settings_query_changed();
        }
        _ => {}
    }
    state.dirty = true;
    true
}

/// Clipboard keys for the search/number field (the app routes ⌘-keys to commands
/// otherwise).
fn clipboard_key(state: &mut AppState, key: Key) -> bool {
    let Key::Character(ch) = key else {
        return false;
    };
    match ch.to_ascii_lowercase() {
        'a' => {
            if let Some(field) = active_field_mut(state) {
                field.select_all();
            }
        }
        'c' => {
            let text =
                active_field(state).and_then(|field| field.selected_text().map(str::to_string));
            if let Some(text) = text {
                state.set_clipboard(&text);
            }
        }
        'x' => {
            let text =
                active_field(state).and_then(|field| field.selected_text().map(str::to_string));
            if let Some(text) = text {
                state.set_clipboard(&text);
                if let Some(field) = active_field_mut(state) {
                    field.backspace();
                }
            }
        }
        'v' => {
            if let Some(text) = state.get_clipboard() {
                if let Some(field) = active_field_mut(state) {
                    field.insert(&text);
                }
            }
        }
        _ => return false,
    }
    true
}

fn active_field(state: &AppState) -> Option<&TextEdit> {
    match state.settings_ui.editing.as_ref() {
        Some(edit) => Some(&edit.field),
        None => Some(&state.settings_ui.query),
    }
}

fn active_field_mut(state: &mut AppState) -> Option<&mut TextEdit> {
    match state.settings_ui.editing.as_mut() {
        Some(edit) => Some(&mut edit.field),
        None => Some(&mut state.settings_ui.query),
    }
}

/// Committed typing and IME commits.
pub fn on_text(state: &mut AppState, text: &str) {
    if !state.settings_open {
        return;
    }
    state.settings_text(text);
}

pub fn on_ime(state: &mut AppState, event: &igui_core::ImeEvent) {
    if !state.settings_open {
        return;
    }
    match event {
        igui_core::ImeEvent::Preedit { text, .. } => {
            state.settings_ui.preedit = Some(text.clone());
            state.dirty = true;
        }
        igui_core::ImeEvent::Commit(text) => {
            state.settings_ui.preedit = None;
            state.settings_text(text);
        }
        igui_core::ImeEvent::Disabled => {
            state.settings_ui.preedit = None;
            state.dirty = true;
        }
        igui_core::ImeEvent::Enabled => {}
    }
}

/// Caret rectangle of the panel's active field, for the host IME placement.
pub fn ime_caret(state: &AppState) -> Rect {
    if state.settings_editing().is_some() {
        state.settings_ui.edit_caret
    } else {
        state.settings_ui.search_caret
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::session::Session;
    use crate::model::Workspace;
    use crate::settings::ROWS;
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
    fn layout_has_a_group_nav_and_one_pane() {
        let mut state = state();
        state.viewport = igui_core::ViewportSize::new(Size::new(1200.0, 900.0));
        state.open_settings();
        let initial = layout(&state);
        // The pane shows the selected row's group; the nav lists every group.
        assert_eq!(initial.nav_items.len(), AppState::settings_groups().len());
        assert_eq!(state.settings_group(), Group::Appearance);
        assert_eq!(initial.rows.len(), 2);
        assert!(initial.groups.is_empty(), "no group headers without search");
        for slot in &initial.rows {
            assert!(
                initial.content.contains_rect(slot.rect),
                "{:?} escapes the content pane",
                slot.spec.row
            );
            assert!(initial.panel.contains_rect(slot.rect));
        }
        for item in &initial.nav_items {
            assert!(initial.nav.contains_rect(item.rect));
            assert!(item.enabled);
        }
        assert!(initial.panel.contains_rect(initial.title));
        assert!(initial.panel.contains_rect(initial.search));
        assert!(initial.panel.contains_rect(initial.reset));
        // Title and search share one row.
        assert_eq!(initial.search.top(), initial.title.top());
        assert!(initial.search.left() > initial.title.right());

        // Searching spans groups and brings the group headers back.
        state.settings_text("size");
        let searched = layout(&state);
        assert_eq!(searched.rows.len(), 1);
        assert_eq!(searched.rows[0].spec.row, Row::FontSize);
        assert_eq!(searched.groups.len(), 1);
        assert_eq!(searched.groups[0].group, Group::Appearance);
    }

    #[test]
    fn narrow_window_keeps_the_panel_inside_and_the_nav_usable() {
        let mut state = state();
        state.viewport = igui_core::ViewportSize::new(Size::new(500.0, 400.0));
        state.open_settings();
        state.settings_show_group(Group::Language);
        let layout = layout(&state);
        let viewport = state.viewport.logical_rect();
        assert!(layout.panel.left() >= viewport.left());
        assert!(layout.panel.right() <= viewport.right());
        assert!(layout.nav.width() >= 100.0);
        assert!(layout.content.width() > 0.0);
        for slot in &layout.rows {
            assert!(slot.rect.left() >= layout.content.left());
            assert!(slot.rect.right() <= layout.content.right());
        }
    }

    #[test]
    fn panel_fits_the_minimum_window() {
        let (width, height) = crate::host::MIN_WINDOW_SIZE;
        let mut state = state();
        state.viewport = igui_core::ViewportSize::new(Size::new(width as f32, height as f32));
        state.open_settings();
        // Worst case: a broad query lists rows from every group.
        state.settings_text("e");
        let layout = layout(&state);
        let viewport = state.viewport.logical_rect();
        assert!(layout.panel.left() >= viewport.left());
        assert!(layout.panel.right() <= viewport.right());
        assert!(layout.panel.top() >= viewport.top());
        assert!(layout.panel.bottom() <= viewport.bottom());
        assert!(layout.max_scroll > 0.0);
        assert!(layout
            .nav_items
            .iter()
            .all(|item| layout.nav.contains_rect(item.rect)));
    }

    #[test]
    fn panel_height_is_capped_on_tall_windows() {
        let mut state = state();
        state.viewport = igui_core::ViewportSize::new(Size::new(1200.0, 1400.0));
        state.open_settings();
        state.settings_show_group(Group::Editor);
        let layout = layout(&state);
        assert!(
            layout.panel.height() <= HEADER_H + MAX_BODY_H + FOOTER_H + 0.5,
            "panel grew past its cap: {}",
            layout.panel.height()
        );
        // The whole Editor group fits without scrolling.
        let last = layout.rows.last().expect("rows");
        assert!(last.rect.bottom() <= layout.content.bottom() + 0.5);
        assert_eq!(layout.max_scroll, 0.0);
    }

    #[test]
    fn small_window_scrolls_instead_of_overflowing() {
        let mut state = state();
        state.viewport = igui_core::ViewportSize::new(Size::new(900.0, 320.0));
        state.open_settings();
        state.settings_show_group(Group::Editor);
        let first = layout(&state);
        assert!(first.max_scroll > 0.0);
        assert!(first.panel.height() <= 320.0);
        state.settings_ui.scroll = first.max_scroll;
        let scrolled = layout(&state);
        let last = scrolled.rows.last().expect("rows");
        assert!(last.rect.bottom() <= scrolled.content.bottom() + 0.5);
    }

    #[test]
    fn search_filters_rows_and_advances_selection() {
        let mut state = state();
        state.open_settings();
        state.settings_text("minimap");
        let visible = state.settings_visible();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].row, Row::Minimap);
        assert_eq!(state.settings_ui.row, Row::Minimap);
        assert_eq!(state.settings_pane().len(), 1);
        state.settings_text("no such setting");
        assert!(state.settings_visible().is_empty());
        assert!(state.settings_pane().is_empty());
        state.settings_ui.query.set_text("");
        state.settings_query_changed();
        assert_eq!(state.settings_visible().len(), ROWS.len());
        // Clearing the query keeps the selection, so the pane is its group.
        assert_eq!(state.settings_group(), Group::View);
        assert!(state
            .settings_pane()
            .iter()
            .all(|spec| spec.group == Group::View));
    }

    #[test]
    fn left_nav_and_tab_switch_groups() {
        let mut state = state();
        state.viewport = igui_core::ViewportSize::new(Size::new(1200.0, 900.0));
        state.open_settings();
        let initial = layout(&state);
        let view = initial
            .nav_items
            .iter()
            .find(|item| item.group == Group::View)
            .expect("View nav item");
        assert!(click(&mut state, view.rect.center()));
        assert_eq!(state.settings_group(), Group::View);
        assert_eq!(state.settings_ui.row, Row::Minimap);

        // Tab cycles groups through the keyboard and keeps the search clear.
        assert!(on_key(&mut state, Key::Tab, Modifiers::NONE));
        assert_eq!(state.settings_group(), Group::Explorer);
        assert!(on_key(
            &mut state,
            Key::Tab,
            Modifiers {
                shift: true,
                ..Modifiers::NONE
            }
        ));
        assert_eq!(state.settings_group(), Group::View);

        // A search result disables the groups it does not appear in.
        state.settings_text("minimap");
        let searched = layout(&state);
        assert!(searched
            .nav_items
            .iter()
            .filter(|item| item.enabled)
            .all(|item| item.group == Group::View));
        assert!(searched
            .nav_items
            .iter()
            .filter(|item| item.group != Group::View)
            .all(|item| !item.enabled));
    }

    #[test]
    fn choice_rows_rotate_and_button_groups_pick_directly() {
        let mut state = state();
        state.open_settings();
        assert_eq!(state.settings_choice_index(Row::Theme), Some(0));
        state.adjust_setting(Row::Theme, 1);
        assert_eq!(state.settings_choice_index(Row::Theme), Some(1));
        state.adjust_setting(Row::Theme, 1);
        assert_eq!(state.settings_choice_index(Row::Theme), Some(0));
        state.settings_set_choice(Row::IndentStyle, 1);
        assert!(state.settings.indent_tabs);
        state.settings_set_choice(Row::IndentStyle, 0);
        assert!(!state.settings.indent_tabs);
        state.adjust_setting(Row::Keymap, -1);
        assert_eq!(state.settings.keymap, crate::keymap::KeymapId::Vim);
    }

    #[test]
    fn number_rows_step_and_accept_typed_values() {
        let mut state = state();
        state.open_settings();
        state.adjust_setting(Row::FontSize, 1);
        assert_eq!(state.settings.font_size, 14.0);
        assert!(state.settings_begin_edit(Row::FontSize));
        state
            .settings_ui
            .editing
            .as_mut()
            .unwrap()
            .field
            .set_text("17");
        assert!(state.settings_commit_edit());
        assert_eq!(state.settings.font_size, 17.0);
        // Above the max: clamped.
        assert!(state.settings_begin_edit(Row::FontSize));
        state
            .settings_ui
            .editing
            .as_mut()
            .unwrap()
            .field
            .set_text("99");
        assert!(state.settings_commit_edit());
        assert_eq!(state.settings.font_size, crate::settings::FONT_MAX);
        // Unparsable text keeps the old value.
        assert!(state.settings_begin_edit(Row::FontSize));
        state
            .settings_ui
            .editing
            .as_mut()
            .unwrap()
            .field
            .set_text("");
        assert!(!state.settings_commit_edit());
        assert_eq!(state.settings.font_size, crate::settings::FONT_MAX);
    }

    #[test]
    fn view_rows_share_the_menu_commands() {
        let mut state = state();
        state.open_settings();
        let minimap = state.session.minimap;
        state.activate_setting(Row::Minimap);
        assert_eq!(state.session.minimap, !minimap);
        state.activate_setting(Row::Minimap);
        assert_eq!(state.session.minimap, minimap);
        let sync = state.sync_scroll;
        state.activate_setting(Row::ScrollSync);
        assert_eq!(state.sync_scroll, !sync);
        assert_eq!(state.session.sync_scroll, state.sync_scroll);
        state.activate_setting(Row::SoftWrap);
        assert!(state.session.soft_wrap);
        state.activate_setting(Row::SoftWrap);
        assert!(!state.session.soft_wrap);
    }

    #[test]
    fn clicking_rows_adjusts_the_right_widget() {
        let mut state = state();
        state.viewport = igui_core::ViewportSize::new(Size::new(1200.0, 900.0));
        state.open_settings();
        let initial = layout(&state);

        let choice = initial
            .rows
            .iter()
            .find(|slot| slot.spec.row == Row::Theme)
            .expect("theme row");
        let (index, button) = choice.options[1];
        assert_eq!(index, 1);
        assert!(click(&mut state, button.center()));
        assert_eq!(state.settings_choice_index(Row::Theme), Some(1));

        let number = initial
            .rows
            .iter()
            .find(|slot| slot.spec.row == Row::FontSize)
            .expect("font row");
        let plus = number.plus.expect("font row has +");
        let before = state.settings.font_size;
        assert!(click(&mut state, plus.center()));
        assert_eq!(state.settings.font_size, before + 1.0);
        assert!(click(&mut state, number.value.center()));
        assert_eq!(state.settings_editing(), Some(Row::FontSize));

        // Another pane's row needs its group selected first.
        state.settings_show_group(Group::Editor);
        let editor = layout(&state);
        let toggle = editor
            .rows
            .iter()
            .find(|slot| slot.spec.row == Row::LineNumbers)
            .expect("line numbers row");
        assert!(click(&mut state, toggle.rect.center()));
        assert!(!state.settings.line_numbers);

        let reset = editor.reset;
        assert!(click(&mut state, reset.center()));
        assert_eq!(state.settings.font_size, 13.0);
        assert_eq!(state.settings_editing(), None);
    }

    #[test]
    fn reset_restores_every_panel_row() {
        let mut state = state();
        state.open_settings();
        state.adjust_setting(Row::FontSize, 2);
        state.adjust_setting(Row::Theme, 1);
        state.activate_setting(Row::Minimap);
        state.activate_setting(Row::SoftWrap);
        state.activate_setting(Row::ScrollSync);
        state.activate_setting(Row::HiddenFiles);
        state.activate_setting(Row::IgnoredFiles);
        state.activate_setting(Row::Keymap);
        state.reset_settings();
        assert_eq!(state.settings, crate::settings::Settings::default());
        assert_eq!(state.settings.keymap, crate::keymap::KeymapId::VsCode);
        assert_eq!(state.settings_choice_index(Row::Theme), Some(0));
        assert!(state.session.minimap);
        assert!(!state.session.soft_wrap);
        assert!(state.sync_scroll);
        assert!(!state.settings.show_hidden);
        assert_eq!(
            state.settings.ignored_files,
            crate::model::ignore::IgnoreMode::Dim
        );
        assert!(state.settings_ui.query.text().is_empty());
        assert_eq!(state.settings_editing(), None);
        assert_eq!(state.settings_group(), Group::Appearance);
    }

    #[test]
    fn clicking_outside_closes_the_panel() {
        let mut state = state();
        state.viewport = igui_core::ViewportSize::new(Size::new(1200.0, 900.0));
        state.open_settings();
        assert!(click(&mut state, Vec2::new(2.0, 2.0)));
        assert!(!state.settings_open);
    }

    #[test]
    fn escape_clears_the_query_before_closing() {
        let mut state = state();
        state.open_settings();
        on_text(&mut state, "wrap");
        assert_eq!(state.settings_ui.query.text(), "wrap");
        assert!(on_key(&mut state, Key::Escape, Modifiers::NONE));
        assert!(state.settings_open);
        assert!(state.settings_ui.query.text().is_empty());
        assert!(on_key(&mut state, Key::Escape, Modifiers::NONE));
        assert!(!state.settings_open);
    }

    #[test]
    fn space_edits_the_query_and_tab_changes_groups() {
        let mut state = state();
        state.open_settings();
        assert_eq!(state.settings_group(), Group::Appearance);
        assert!(on_key(&mut state, Key::Tab, Modifiers::NONE));
        assert_eq!(state.settings_group(), Group::Editor);
        assert!(on_key(&mut state, Key::Space, Modifiers::NONE));
        assert_eq!(state.settings_ui.query.text(), " ");
        assert!(state
            .settings_visible()
            .iter()
            .all(|spec| spec.row.label().contains(' ')));
    }

    #[test]
    fn typing_a_number_routes_digits_to_the_edit_field() {
        let mut state = state();
        state.open_settings();
        state.activate_setting(Row::FontSize);
        assert_eq!(state.settings_editing(), Some(Row::FontSize));
        state.settings_text("1x6");
        let field = &state.settings_ui.editing.as_ref().expect("editing").field;
        assert_eq!(field.text(), "16");
        assert!(on_key(&mut state, Key::Enter, Modifiers::NONE));
        assert_eq!(state.settings.font_size, 16.0);
    }
}
