//! Benchmarks for the Markdown preview pipeline (`--features bench`).
//!
//! The windowed app is exercised through the same `EditorApp` methods the
//! headless self-check uses, so this measures real parsing, layout, and paint
//! command generation without an OS window. Run with:
//!
//! ```bash
//! cargo bench --features bench --bench markdown
//! ```

use std::time::{Duration, Instant};

use igui_core::{FontWeight, Size, ViewportSize};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::app::EditorApp;
use crate::dialogs::ScriptedDialogs;
use crate::model::markdown;
use crate::theme::{Mode, Theme};
use crate::ui::preview;

/// One benchmark result.
#[derive(Clone, Copy, Debug)]
pub struct Report {
    pub iterations: usize,
    pub elapsed: Duration,
    /// Average milliseconds per iteration.
    pub average_ms: f64,
    /// Primitives in the last layout (zero for paint-only runs).
    pub primitives: usize,
    /// Draw commands in the last recorded frame (zero for layout-only runs).
    pub commands: usize,
}

impl Report {
    fn new(iterations: usize, elapsed: Duration) -> Self {
        Self {
            iterations,
            elapsed,
            average_ms: elapsed.as_secs_f64() * 1000.0 / iterations.max(1) as f64,
            primitives: 0,
            commands: 0,
        }
    }
}

/// Deterministic measurer for layout benchmarks: no font I/O, stable numbers.
struct BenchMeasurer;

impl TextMeasurer for BenchMeasurer {
    fn advance(&self, ch: char, font_size: f32) -> f32 {
        if ch == ' ' {
            font_size * 0.25
        } else if ch == '中' {
            font_size
        } else {
            font_size * 0.55
        }
    }

    fn line_height(&self, font_size: f32) -> f32 {
        font_size * 1.4
    }

    fn measure_run_weighted(&self, text: &str, font_size: f32, weight: FontWeight) -> f32 {
        let _ = weight;
        text.chars().map(|ch| self.advance(ch, font_size)).sum()
    }

    fn measure_run_weighted_in(
        &self,
        text: &str,
        font_size: f32,
        weight: FontWeight,
        family: Option<&str>,
    ) -> f32 {
        let _ = family;
        self.measure_run_weighted(text, font_size, weight)
    }
}

/// Parses Markdown `iterations` times (pulldown-cmark + block folding).
pub fn markdown_parse(text: &str, iterations: usize) -> Report {
    let iterations = iterations.max(1);
    let start = Instant::now();
    let mut primitives = 0;
    for _ in 0..iterations {
        primitives = markdown::parse(text).len();
    }
    let mut report = Report::new(iterations, start.elapsed());
    report.primitives = primitives;
    report
}

/// Parses once, then lays the preview out `iterations` times.
pub fn markdown_layout(text: &str, width: f32, iterations: usize) -> Report {
    let iterations = iterations.max(1);
    let blocks = markdown::parse(text);
    let theme = Theme::dark();
    let measurer = BenchMeasurer;
    let start = Instant::now();
    let mut primitives = 0;
    for _ in 0..iterations {
        let (prims, _, _) = preview::layout(&blocks, &measurer, width, &theme, 13.0);
        primitives = prims.len();
    }
    let mut report = Report::new(iterations, start.elapsed());
    report.primitives = primitives;
    report
}

/// Simulates typing in a Markdown document at `frame_ms` intervals.
///
/// Each frame inserts one character and runs the app pipeline; the reported
/// time is the CPU work per frame (sleep is excluded). Large documents coalesce
/// their preview rebuild, so this is the regression guard for that path.
pub fn markdown_typing(text: &str, frames: usize, frame_ms: u64) -> Result<Report, String> {
    let frames = frames.max(1);
    let dir = std::env::temp_dir().join(format!("igui-editor-bench-type-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|error| format!("fixture dir: {error}"))?;
    let file = dir.join("bench.md");
    std::fs::write(&file, text).map_err(|error| format!("fixture: {error}"))?;
    let config = dir.join("session.conf");
    let mut app = EditorApp::with_config(
        Some(file),
        Some(Mode::Dark),
        config,
        Box::new(ScriptedDialogs::default()),
    );
    let viewport = ViewportSize::new(Size::new(1200.0, 800.0));
    app.update(0.0);
    app.layout(viewport);
    let mut warmup = PaintContext::new();
    app.paint(&mut warmup);
    let _ = warmup.into_draw_list();

    let mut work = Duration::ZERO;
    for _ in 0..frames {
        if let Some(doc) = app.state_mut().ws.active_doc_mut() {
            doc.insert_text("x");
        }
        let start = Instant::now();
        app.update(0.0);
        app.layout(viewport);
        let mut ctx = PaintContext::new();
        app.paint(&mut ctx);
        let _ = ctx.into_draw_list();
        work += start.elapsed();
        std::thread::sleep(Duration::from_millis(frame_ms));
    }
    // Let the trailing debounce fire so the benchmark does not leave a pending
    // rebuild behind; this final layout is not part of the per-frame number.
    std::thread::sleep(Duration::from_millis(150));
    app.update(0.0);
    app.layout(viewport);
    let mut final_ctx = PaintContext::new();
    app.paint(&mut final_ctx);
    let commands = final_ctx.into_draw_list().len();

    let mut report = Report::new(frames, work);
    report.commands = commands;
    let _ = std::fs::remove_dir_all(dir);
    Ok(report)
}

/// Drives the real app for `frames` frames and counts generated draw commands.
///
/// The Markdown file opens in Split view; the first frame builds the preview
/// cache, the remaining frames measure the steady-state paint path.
pub fn markdown_paint(text: &str, frames: usize) -> Result<Report, String> {
    let frames = frames.max(1);
    let dir = std::env::temp_dir().join(format!("igui-editor-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|error| format!("fixture dir: {error}"))?;
    let file = dir.join("bench.md");
    std::fs::write(&file, text).map_err(|error| format!("fixture: {error}"))?;
    let config = dir.join("session.conf");
    let mut app = EditorApp::with_config(
        Some(file),
        Some(Mode::Dark),
        config,
        Box::new(ScriptedDialogs::default()),
    );
    let viewport = ViewportSize::new(Size::new(1200.0, 800.0));

    // Warm up: shell build + parse + first layout + cache.
    app.update(0.0);
    app.layout(viewport);
    let mut warmup = PaintContext::new();
    app.paint(&mut warmup);
    let _ = warmup.into_draw_list();

    let start = Instant::now();
    let mut commands = 0;
    for _ in 0..frames {
        app.update(0.0);
        app.layout(viewport);
        let mut ctx = PaintContext::new();
        app.paint(&mut ctx);
        let list = ctx.into_draw_list();
        commands = list.len();
    }
    let mut report = Report::new(frames, start.elapsed());
    report.commands = commands;
    let _ = std::fs::remove_dir_all(dir);
    Ok(report)
}
