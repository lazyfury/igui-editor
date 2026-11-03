//! Low-level view plumbing: one place that knows how to make a control node
//! and paint text.
//!
//! The editor deliberately does **not** use `igui_components`: its look is
//! compact and square-cornered, and every widget here is app-specific anyway
//! (gutter, tab strip, file tree). What it does reuse is the engine:
//! `igui_ui`'s layout, input routing, clipping, text measurement and
//! `igui_render`'s draw list.

use igui_core::{
    Color, EventResult, FontWeight, ImeEvent, Key, Modifiers, NodeId, Rect, Size, Vec2,
};
use igui_render::{PaintContext, TextAlign};
use igui_scene::SceneTree;
use igui_ui::{
    Container, Control, ControlContent, ControlData, FlexStyle, InteractState, LayoutStyle,
    PaintEnv, PointerPhase, SizeBasis, TextMeasurer,
};

/// `Rect` has `size.width` / `size.height`; the painters read `width()` /
/// `height()` everywhere, so the trait keeps them readable.
pub trait RectExt {
    fn width(&self) -> f32;
    fn height(&self) -> f32;
}

impl RectExt for Rect {
    fn width(&self) -> f32 {
        self.size.width
    }

    fn height(&self) -> f32 {
        self.size.height
    }
}

/// Draw text with its baseline at `baseline`.
pub fn text(
    ctx: &mut PaintContext,
    value: &str,
    x: f32,
    baseline: f32,
    font_size: f32,
    color: Color,
) {
    if value.is_empty() {
        return;
    }
    ctx.draw_text(
        value,
        Vec2::new(x, baseline),
        font_size,
        TextAlign::Left,
        color,
    );
}

/// Right-aligned text ending at `right`.
pub fn text_right(
    ctx: &mut PaintContext,
    measurer: &dyn TextMeasurer,
    value: &str,
    right: f32,
    baseline: f32,
    font_size: f32,
    color: Color,
) {
    let width = measurer.measure_run_weighted(value, font_size, FontWeight::NORMAL);
    text(ctx, value, right - width, baseline, font_size, color);
}

/// Horizontally centered text inside `rect`.
pub fn text_center(
    ctx: &mut PaintContext,
    measurer: &dyn TextMeasurer,
    value: &str,
    rect: Rect,
    baseline: f32,
    font_size: f32,
    color: Color,
) {
    let width = measurer.measure_run_weighted(value, font_size, FontWeight::NORMAL);
    text(
        ctx,
        value,
        rect.center().x - width * 0.5,
        baseline,
        font_size,
        color,
    );
}

pub fn width(measurer: &dyn TextMeasurer, value: &str, font_size: f32) -> f32 {
    measurer.measure_run_weighted(value, font_size, FontWeight::NORMAL)
}

/// [`width`] with an explicit weight (bold runs are wider).
pub fn width_weighted(
    measurer: &dyn TextMeasurer,
    value: &str,
    font_size: f32,
    weight: FontWeight,
) -> f32 {
    measurer.measure_run_weighted(value, font_size, weight)
}

/// Baseline that vertically centers one line of `font_size` inside `rect`.
pub fn baseline(measurer: &dyn TextMeasurer, rect: Rect, font_size: f32) -> f32 {
    rect.center().y + measurer.ascent(font_size) * 0.5 - font_size * 0.08
}

/// `value` cut to at most `max_width`, with a trailing `…` when truncated.
pub fn truncate(
    measurer: &dyn TextMeasurer,
    value: &str,
    font_size: f32,
    max_width: f32,
) -> String {
    if width(measurer, value, font_size) <= max_width {
        return value.to_string();
    }
    let ellipsis = "…";
    let budget = (max_width - width(measurer, ellipsis, font_size)).max(0.0);
    let mut out = String::new();
    let mut used = 0.0;
    for ch in value.chars() {
        let advance = measurer.advance(ch, font_size);
        if used + advance > budget {
            break;
        }
        out.push(ch);
        used += advance;
    }
    out.push_str(ellipsis);
    out
}

/// `value` cut from the left (keeps the tail — paths, long identifiers).
pub fn truncate_left(
    measurer: &dyn TextMeasurer,
    value: &str,
    font_size: f32,
    max_width: f32,
) -> String {
    if width(measurer, value, font_size) <= max_width {
        return value.to_string();
    }
    let ellipsis = "…";
    let budget = (max_width - width(measurer, ellipsis, font_size)).max(0.0);
    let mut used = 0.0;
    let mut start = value.len();
    for (index, ch) in value.char_indices().rev() {
        let advance = measurer.advance(ch, font_size);
        if used + advance > budget {
            break;
        }
        used += advance;
        start = index;
    }
    format!("{ellipsis}{}", &value[start..])
}

pub fn fill(ctx: &mut PaintContext, rect: Rect, color: Color) {
    ctx.fill_rect(rect, color);
}

pub fn hline(ctx: &mut PaintContext, x0: f32, x1: f32, y: f32, color: Color) {
    if x1 > x0 {
        ctx.fill_rect(
            Rect::from_min_size(Vec2::new(x0, y), Size::new(x1 - x0, 1.0)),
            color,
        );
    }
}

pub fn vline(ctx: &mut PaintContext, x: f32, y0: f32, y1: f32, color: Color) {
    if y1 > y0 {
        ctx.fill_rect(
            Rect::from_min_size(Vec2::new(x, y0), Size::new(1.0, y1 - y0)),
            color,
        );
    }
}

// -- node construction -------------------------------------------------------

/// Adds a control node with default data (no content, leaf container).
pub fn node(tree: &mut SceneTree, parent: NodeId, name: &str) -> NodeId {
    let id = tree.add_control(parent, name);
    tree.set_data(
        id,
        Control::new(ControlData::default(), Container::Leaf, None),
    );
    id
}

/// A control's self-draw closure (no tree access: paint reads shared state).
pub type Painter = Box<dyn Fn(&mut PaintEnv<'_>, Rect, InteractState)>;

/// Installs a self-drawing content closure on `id`.
pub fn draw(
    tree: &mut SceneTree,
    id: NodeId,
    painter: impl Fn(&mut PaintEnv<'_>, Rect, InteractState) + 'static,
) {
    if let Some(control) = tree.data_mut::<Control>(id) {
        control.content = Some(Box::new(DrawContent {
            painter: Box::new(painter),
        }));
    }
}

struct DrawContent {
    painter: Painter,
}

impl ControlContent for DrawContent {
    fn draw(&self, _id: NodeId, env: &mut PaintEnv<'_>, rect: Rect, state: InteractState) {
        (self.painter)(env, rect, state);
    }
}

/// Sets the flex participation of `id`.
pub fn flex(tree: &mut SceneTree, id: NodeId, grow: f32, basis_px: Option<f32>, shrink: f32) {
    if let Some(control) = tree.data_mut::<Control>(id) {
        control.data.layout = LayoutStyle {
            grow,
            shrink,
            basis: basis_px.map_or(SizeBasis::Auto, SizeBasis::Px),
            ..LayoutStyle::default()
        };
    }
}

/// Sets the minimum size of `id`.
pub fn min_size(tree: &mut SceneTree, id: NodeId, width: f32, height: f32) {
    if let Some(control) = tree.data_mut::<Control>(id) {
        control.data.min_size = Size::new(width, height);
    }
}

pub fn clip(tree: &mut SceneTree, id: NodeId, clip: bool) {
    if let Some(control) = tree.data_mut::<Control>(id) {
        control.data.clip = clip;
    }
}

pub fn set_container(tree: &mut SceneTree, id: NodeId, container: Container) {
    if let Some(control) = tree.data_mut::<Control>(id) {
        control.container = container;
    }
}

/// A tight flex container: zero padding, zero gap (`editable_column` style).
pub fn tight_row() -> Container {
    Container::Flex(FlexStyle::row().padding(igui_core::Edges::ZERO).gap(0.0))
}

pub fn tight_column() -> Container {
    Container::Flex(FlexStyle::column().padding(igui_core::Edges::ZERO).gap(0.0))
}

pub fn focusable(tree: &mut SceneTree, id: NodeId) {
    if let Some(control) = tree.data_mut::<Control>(id) {
        control.focusable = true;
    }
}

pub fn on_key(
    tree: &mut SceneTree,
    id: NodeId,
    callback: impl FnMut(&mut SceneTree, NodeId, Key, bool, Modifiers) -> EventResult + 'static,
) {
    igui_ui::set_key_callback(tree, id, callback);
}

pub fn on_text(
    tree: &mut SceneTree,
    id: NodeId,
    callback: impl FnMut(&mut SceneTree, NodeId, &str) + 'static,
) {
    igui_ui::set_text_callback(tree, id, callback);
}

pub fn on_ime(
    tree: &mut SceneTree,
    id: NodeId,
    callback: impl FnMut(&mut SceneTree, NodeId, &ImeEvent) + 'static,
) {
    igui_ui::set_ime_callback(tree, id, callback);
}

pub fn on_pointer(
    tree: &mut SceneTree,
    id: NodeId,
    callback: impl FnMut(&mut SceneTree, NodeId, PointerPhase, Rect, Vec2) + 'static,
) {
    igui_ui::set_pointer_callback(tree, id, callback);
}

pub fn on_scroll(
    tree: &mut SceneTree,
    id: NodeId,
    callback: impl FnMut(&mut SceneTree, NodeId, Vec2) + 'static,
) {
    if let Some(control) = tree.data_mut::<Control>(id) {
        control.scroll_callback = Some(std::rc::Rc::new(std::cell::RefCell::new(callback)));
    }
}

pub fn set_caret_provider(
    tree: &mut SceneTree,
    id: NodeId,
    provider: impl Fn() -> Option<Rect> + 'static,
) {
    if let Some(control) = tree.data_mut::<Control>(id) {
        control.caret_provider = Some(std::rc::Rc::new(provider));
    }
}

pub fn set_cursor(tree: &mut SceneTree, id: NodeId, cursor: igui_core::Cursor) {
    if let Some(control) = tree.data_mut::<Control>(id) {
        control.data.cursor = cursor;
    }
}
