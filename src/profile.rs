//! Optional runtime performance overlay (`profile` feature).
//!
//! This mirrors `examples/wgpu_demo`: [`igui_profile`] measures where a frame
//! went, [`igui_debug_ui::PerformanceOverlay`] paints it, and a
//! [`FrameObserver`] records the render/present tail after the presenter has
//! run. The feature is **not** part of the shipped bundle (packaging uses
//! `--no-default-features`):
//!
//! ```bash
//! cargo run --features profile -- --profile README.md
//! ```
//!
//! F4 toggles the panel at runtime. The overlay owns its own `Ui` tree, so it
//! does not disturb the editor's layout or hit-testing; while it is open it
//! consumes pointer/keys over its panel.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use igui_app::{App, FrameObserver};
use igui_core::{EventResult, InputEvent, Size, ViewportSize};
use igui_debug_ui::PerformanceOverlay;
use igui_profile::{inspect, FrameCounters, FrameStats, InspectionReport, Profiler, StageTimes};
use igui_render::{DrawList, PaintContext};

/// Editor frame timings plus the overlay and the last inspection report.
pub struct ProfileState {
    profiler: Profiler,
    report: InspectionReport,
    overlay: PerformanceOverlay,
    frame_start: Instant,
    update_start: Instant,
    layout_start: Instant,
    paint_start: Instant,
    update_ms: f32,
    layout_ms: f32,
    paint_ms: f32,
    paint_done: Instant,
    controls: usize,
    nodes: usize,
}

impl ProfileState {
    pub fn new(open: bool) -> Self {
        let mut overlay = PerformanceOverlay::new();
        overlay.set_open(open);
        let now = Instant::now();
        Self {
            profiler: Profiler::new(),
            report: InspectionReport::new(),
            overlay,
            frame_start: now,
            update_start: now,
            layout_start: now,
            paint_start: now,
            update_ms: 0.0,
            layout_ms: 0.0,
            paint_ms: 0.0,
            paint_done: now,
            controls: 0,
            nodes: 0,
        }
    }

    pub fn toggle(&mut self) {
        self.overlay.toggle();
    }

    pub fn handle_input(&mut self, event: &InputEvent) -> EventResult {
        self.overlay.handle_input(event)
    }

    /// Starts the update phase and the whole frame.
    pub fn begin_frame(&mut self) {
        let now = Instant::now();
        self.frame_start = now;
        self.update_start = now;
        self.paint_done = now;
    }

    pub fn end_update(&mut self) {
        let now = Instant::now();
        self.update_ms = millis(now - self.update_start);
        self.layout_start = now;
    }

    pub fn end_layout(&mut self) {
        self.layout_ms = millis(Instant::now() - self.layout_start);
    }

    pub fn begin_paint(&mut self) {
        self.paint_start = Instant::now();
    }

    /// Ends the app paint (before the overlay itself is painted).
    pub fn end_paint(&mut self, controls: usize, nodes: usize) {
        let now = Instant::now();
        self.paint_ms = millis(now - self.paint_start);
        self.paint_done = now;
        self.controls = controls;
        self.nodes = nodes;
    }

    /// Paints the overlay after the app and its own overlays.
    pub fn paint(&mut self, ctx: &mut PaintContext, viewport: ViewportSize) {
        self.overlay.update(&self.profiler, &self.report, viewport);
        self.overlay.paint(ctx);
    }

    /// Records the frame after the presenter has run.
    pub fn after_frame(&mut self, list: &DrawList) {
        let now = Instant::now();
        let stats = FrameStats {
            index: self.profiler.next_index(),
            frame_ms: millis(now - self.frame_start),
            stages: StageTimes::new(
                self.update_ms,
                self.layout_ms,
                self.paint_ms,
                millis(now - self.paint_done),
            ),
            counters: FrameCounters::new(self.nodes, self.controls, list.len(), 1),
        };
        self.profiler.record(stats);
        if self.overlay.is_open() {
            self.report = inspect(list, &stats);
        }
    }
}

/// Records the render/present tail once the frame has been presented.
pub struct ProfileObserver {
    state: Rc<RefCell<ProfileState>>,
}

impl ProfileObserver {
    pub fn new(state: Rc<RefCell<ProfileState>>) -> Self {
        Self { state }
    }
}

impl FrameObserver for ProfileObserver {
    fn after_frame(&mut self, _app: &App, list: &DrawList) {
        self.state.borrow_mut().after_frame(list);
    }
}

fn millis(duration: Duration) -> f32 {
    duration.as_secs_f32() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_open_panel_paints_commands() {
        let mut state = ProfileState::new(true);
        state.begin_frame();
        state.end_update();
        state.end_layout();
        state.begin_paint();
        state.end_paint(10, 20);
        state.after_frame(&DrawList::default());
        let mut ctx = PaintContext::new();
        state.paint(&mut ctx, ViewportSize::new(Size::new(1000.0, 700.0)));
        assert!(
            !ctx.into_draw_list().commands().is_empty(),
            "the open performance panel painted nothing"
        );
    }

    #[test]
    fn a_frame_is_recorded_and_the_panel_toggles() {
        let mut state = ProfileState::new(false);
        state.begin_frame();
        state.end_update();
        state.end_layout();
        state.begin_paint();
        state.end_paint(10, 20);
        state.after_frame(&DrawList::default());
        assert_eq!(state.profiler.len(), 1);
        assert!(!state.overlay.is_open());
        state.toggle();
        assert!(state.overlay.is_open());
        state.toggle();
        assert!(!state.overlay.is_open());
    }
}
