//! The one drag-handle specification (see `DESIGN.md` §“Drag handle”).
//!
//! A 4 px hit strip that paints:
//!
//! - a panel-tinted background (`panel` → `hover` → accent 16% while dragging),
//! - one 1 px line centered on the strip (`border` → `dim` → `accent`),
//! - a 2 × 28 grip nudged 1 px toward the app centre (`GripSide::Right` for the
//!   left-side sidebar handle, `GripSide::Left` for the split handle),
//!   `faint` → `text` → `accent`.
//!
//! Both handles pass the shared content-band centre as the grip's `y`, so the
//! two grips always sit at the same height (even with the find bar open); the
//! value is clamped into the handle. `None` falls back to the handle's own
//! centre (before the first layout).
//!
//! Exactly one line per seam: the pane that the handle separates must not paint
//! its own border.
//!
//! The cursor is `ColResize`; callers own the drag math (sidebar width, split
//! ratio) and decide whether double-click has a meaning.

use igui_core::{Cursor, Rect, Size, Vec2};
use igui_ui::PaintEnv;

use crate::kit::{self, RectExt};
use crate::theme::Theme;

/// Hit area width (and the space the handle occupies in layout).
pub const HANDLE_WIDTH: f32 = 4.0;
/// Grip bar width.
pub const GRIP_WIDTH: f32 = 2.0;
/// Grip bar height (clamped to half the handle height for short panels).
pub const GRIP_HEIGHT: f32 = 28.0;
/// The grip is nudged this far toward the app centre.
pub const GRIP_OFFSET: f32 = 1.0;

/// Which way the grip leans: toward the app centre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GripSide {
    /// Left-side handle (sidebar): nudge right.
    Right,
    /// Right-side handle (split): nudge left.
    Left,
    /// Top-edge handle (terminal): nudge down.
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Vertical,
    Horizontal,
}

/// Grip centre for a handle centred at `center_x`.
pub fn grip_center_x(center_x: f32, side: GripSide) -> f32 {
    match side {
        GripSide::Right => center_x + GRIP_OFFSET,
        GripSide::Left => center_x - GRIP_OFFSET,
        GripSide::Down => center_x,
    }
}

/// Grip centre for a horizontal handle centered at `center_y`.
pub fn grip_center_y(center_y: f32, side: GripSide) -> f32 {
    match side {
        GripSide::Down => center_y + GRIP_OFFSET,
        GripSide::Right | GripSide::Left => center_y,
    }
}

/// Cursor the host shows while hovering the handle.
pub const fn cursor() -> Cursor {
    Cursor::ColResize
}

pub const fn cursor_for(axis: Axis) -> Cursor {
    match axis {
        Axis::Vertical => Cursor::ColResize,
        Axis::Horizontal => Cursor::RowResize,
    }
}

/// Paints the handle for its interaction state.
#[allow(clippy::too_many_arguments)]
pub fn paint(
    env: &mut PaintEnv<'_>,
    rect: Rect,
    theme: &Theme,
    hovered: bool,
    dragging: bool,
    axis: Axis,
    grip_side: GripSide,
    grip_center: Option<f32>,
) {
    let p = theme.p;
    let background = if dragging {
        p.accent.alpha_mul(0.16)
    } else if hovered {
        p.hover
    } else {
        p.panel
    };
    kit::fill(env.ctx, rect, background);

    let line = if dragging {
        p.accent
    } else if hovered {
        p.dim
    } else {
        p.border
    };
    let grip = match axis {
        Axis::Vertical => {
            kit::vline(
                env.ctx,
                (rect.center().x - 0.5).round(),
                rect.top(),
                rect.bottom(),
                line,
            );
            let grip_height = GRIP_HEIGHT.min(rect.height() * 0.5);
            let half = grip_height * 0.5;
            let min_y = rect.top() + half;
            let max_y = rect.bottom() - half;
            let center_y = match grip_center {
                Some(y) if max_y >= min_y => y.clamp(min_y, max_y),
                _ => rect.center().y,
            };
            Rect::from_center_size(
                Vec2::new(grip_center_x(rect.center().x, grip_side), center_y),
                Size::new(GRIP_WIDTH, grip_height),
            )
        }
        Axis::Horizontal => {
            kit::hline(
                env.ctx,
                rect.left(),
                rect.right(),
                (rect.center().y - 0.5).round(),
                line,
            );
            let grip_width = GRIP_HEIGHT.min(rect.width() * 0.5);
            let half = grip_width * 0.5;
            let min_x = rect.left() + half;
            let max_x = rect.right() - half;
            let center_x = match grip_center {
                Some(x) if max_x >= min_x => x.clamp(min_x, max_x),
                _ => rect.center().x,
            };
            Rect::from_center_size(
                Vec2::new(center_x, grip_center_y(rect.center().y, grip_side)),
                Size::new(grip_width, GRIP_WIDTH),
            )
        }
    };
    let grip_color = if dragging {
        p.accent
    } else if hovered {
        p.text
    } else {
        p.faint
    };
    kit::fill(env.ctx, grip, grip_color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_follow_the_spec() {
        assert_eq!(HANDLE_WIDTH, 4.0);
        assert_eq!(GRIP_WIDTH, 2.0);
        assert_eq!(GRIP_HEIGHT, 28.0);
        assert_eq!(GRIP_OFFSET, 1.0);
        assert_eq!(grip_center_x(100.0, GripSide::Right), 101.0);
        assert_eq!(grip_center_x(100.0, GripSide::Left), 99.0);
        assert_eq!(cursor(), Cursor::ColResize);
    }
}
