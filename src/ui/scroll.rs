//! One wheel convention for scroll containers.
//!
//! `InputEvent::Wheel` uses `delta.y > 0` for "scroll down". Every container's
//! offset grows downward, so all handlers must add `delta.y` before clamping;
//! this module keeps that sign from being inverted independently in each view.

/// Applies one wheel delta to a vertical scroll offset.
pub fn apply(offset: &mut f32, delta_y: f32, max: f32) {
    *offset = (*offset + delta_y).clamp(0.0, max.max(0.0));
}

/// Clamps a scroll offset after content/layout changes.
pub fn clamp(offset: &mut f32, max: f32) {
    *offset = offset.clamp(0.0, max.max(0.0));
}
