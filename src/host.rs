//! Native window host: `winit` window + `wgpu` surface via the `igui_winit`
//! platform plugins. The app itself stays host-agnostic (see `app.rs`).

use std::path::{Path, PathBuf};

use igui_app::{App, AppBuilder, AppConfig, PlatformEvent, PlatformObserver, Plugin};
use igui_backend_wgpu::FontConfig;
use igui_core::InputEvent;
use igui_font::FaceRef;
use igui_winit::{
    ClipboardPlugin, GpuConfig, ImePlugin, KeyboardPlugin, PointerPlugin, SharedWindow,
    TextMeasurePlugin, TitlebarMode, WgpuPlugin, WindowConfig, WinitPlugin,
};
use winit::event::WindowEvent;

use crate::app::EditorApp;
use crate::dialogs::{DialogFuture, Dialogs};
use crate::drop::{self, SharedDropTarget};
use crate::model::session::{self, Session};
use crate::theme::{Mode, Theme};

/// Candidate monospace faces, first hit wins. The whole UI uses it — the geek
/// look wants one grid, not a proportional UI font next to a mono editor.
const MONO_FACES: &[&str] = &[
    "/System/Library/Fonts/SFNSMono.ttf",
    "/System/Library/Fonts/Menlo.ttc",
    "/Library/Fonts/JetBrainsMono-Regular.ttf",
    "/Library/Fonts/FiraCode-Regular.ttf",
    "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Regular.ttf",
    "/usr/share/fonts/truetype/firacode/FiraCode-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
];

/// Font configuration: the first installed monospace face becomes the default
/// (fallback discovery still runs for CJK when a character is uncovered).
pub fn font_config() -> FontConfig {
    let mut config = FontConfig {
        device_pixel_rasterization: true,
        ..FontConfig::default()
    };
    for path in MONO_FACES {
        if std::path::Path::new(path).is_file() {
            config.default_face = Some(FaceRef::new(*path, 0));
            break;
        }
    }
    config
}

/// Native file dialogs (`rfd`, asynchronous).
///
/// The parent window matters: on macOS the async API attaches an
/// `NSOpenPanel`/`NSSavePanel` **sheet** to it, so the winit event loop keeps
/// running and the app handler is never re-entered (the blocking API would run
/// a nested modal loop). Linux uses the XDG portal, Windows its common dialogs.
pub struct NativeDialogs {
    window: SharedWindow,
}

impl NativeDialogs {
    fn dialog(&self, start: Option<&Path>) -> rfd::AsyncFileDialog {
        let mut dialog = rfd::AsyncFileDialog::new();
        if let Some(window) = self.window.borrow().clone() {
            dialog = dialog.set_parent(&*window);
        }
        if let Some(dir) = start {
            dialog = dialog.set_directory(dir);
        }
        dialog
    }
}

impl Dialogs for NativeDialogs {
    fn open_file(&mut self, start: Option<&Path>) -> DialogFuture {
        let dialog = self.dialog(start);
        Box::pin(async move {
            dialog
                .pick_file()
                .await
                .map(|handle| handle.path().to_path_buf())
        })
    }

    fn open_folder(&mut self, start: Option<&Path>) -> DialogFuture {
        let dialog = self.dialog(start);
        Box::pin(async move {
            dialog
                .pick_folder()
                .await
                .map(|handle| handle.path().to_path_buf())
        })
    }

    fn save_file(&mut self, start: Option<&Path>, name: &str) -> DialogFuture {
        let dialog = self.dialog(start).set_file_name(name);
        Box::pin(async move {
            dialog
                .save_file()
                .await
                .map(|handle| handle.path().to_path_buf())
        })
    }
}

/// Bridges winit's file drag & drop into the app's shared drop target.
pub struct DropPlugin;

impl Plugin for DropPlugin {
    fn name(&self) -> &'static str {
        "drop"
    }

    fn build(&self, app: &mut AppBuilder) {
        let state = drop::shared();
        app.insert_service(state.clone());
        app.add_platform_observer(DropObserver { state });
    }
}

struct DropObserver {
    state: SharedDropTarget,
}

impl PlatformObserver for DropObserver {
    fn on_platform(&mut self, event: PlatformEvent<'_>, _out: &mut Vec<InputEvent>) {
        let Some(event) = event.downcast_ref::<WindowEvent>() else {
            return;
        };
        let mut state = self.state.borrow_mut();
        match event {
            WindowEvent::HoveredFile(_) => state.hovered = true,
            WindowEvent::HoveredFileCancelled => state.hovered = false,
            WindowEvent::DroppedFile(path) => {
                state.hovered = false;
                state.paths.push(path.to_path_buf());
            }
            _ => {}
        }
    }
}

/// Builds the windowed app (also used by the self-check to prove the wiring).
pub fn build(open: Option<PathBuf>, mode: Option<Mode>) -> App {
    let session = Session::load(&session::path());
    let theme_mode = mode.unwrap_or(if session.is_dark() {
        Mode::Dark
    } else {
        Mode::Light
    });
    let theme = Theme::new(theme_mode);
    let (width, height) = session
        .window
        .map(|geometry| geometry.size)
        .unwrap_or((1200.0, 800.0));
    let builder = App::new(AppConfig {
        title: "igui-editor".into(),
        size: (width as f64, height as f64),
        ..Default::default()
    })
    .plugin(WinitPlugin::new(WindowConfig {
        title: "igui-editor".into(),
        size: (width as f64, height as f64),
        titlebar: TitlebarMode::Native,
        ime: true,
    }))
    .plugin(WgpuPlugin::new(GpuConfig {
        font: font_config(),
        clear_color: theme.p.bg,
        ..Default::default()
    }))
    .plugin(PointerPlugin)
    .plugin(KeyboardPlugin)
    .plugin(ImePlugin)
    .plugin(TextMeasurePlugin)
    .plugin(ClipboardPlugin)
    .plugin(DropPlugin);
    // The window service is registered by `WinitPlugin`; dialogs attach their
    // sheets to it once it is filled in on resume.
    let window = builder
        .service::<SharedWindow>()
        .cloned()
        .unwrap_or_default();
    let logic = EditorApp::with_dialogs(open, mode, Box::new(NativeDialogs { window }));
    builder.logic(logic).build()
}

/// Runs the editor until the window closes.
pub fn run(open: Option<PathBuf>, mode: Option<Mode>) {
    build(open, mode).run();
}
