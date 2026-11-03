//! `igui-editor` — a compact, square-cornered text / Markdown editor.
//!
//! ```text
//! Input -> SceneTree -> Update -> Layout -> Paint -> DrawList -> wgpu
//! ```
//!
//! - `model/`: pure document + workspace logic (unit-tested, no UI).
//! - `state/`: shared app state written by callbacks and read by painters
//!   (one domain implementation per child module).
//! - `ui/`: one module per shell region (menubar, sidebar, tabs, editor, status).
//! - `app.rs`: shell construction, input routing, `igui_app` glue.
//! - `host.rs`: winit + wgpu plugins (native only).
//! - `selfcheck.rs`: headless pipeline assertions (`--selfcheck`), no screenshots.

use std::path::PathBuf;

use igui_editor::host;
#[cfg(feature = "selfcheck")]
use igui_editor::selfcheck;
use igui_editor::theme::Mode;

const HELP: &str = "\
igui-editor — a compact text/Markdown editor built on igui

用法:
  igui-editor [文件] [选项]

选项:
      --light        使用浅色主题（默认遵循上次会话，首次为深色 geek 主题）
      --dark         强制深色主题
      --selfcheck    无头自检：录制 DrawList 并断言结构与文字，不开窗、不截图
  -h, --help         显示本帮助
  -V, --version      显示版本

快捷键:
  ⌘S 保存     ⌘N 新建     ⇧⌘N 新建文件     ⌘W 关闭标签
  ⌘Z 撤销     ⇧⌘Z 重做     ⌥Z 自动换行     ⌃` 终端
  ⌘X / ⌘C / ⌘V 剪切 / 复制 / 粘贴     ⌘A 全选
  ⌘B 侧栏     ⇧⌘L 明暗主题
  Tab / ⇧Tab 缩进 / 反缩进     ⌘← / ⌘→ 行首 / 行尾     ⌘↑ / ⌘↓ 文档首 / 尾

说明:
  侧栏点目录展开、点文件打开；标签页的 × 关闭标签（有未保存修改时拒绝关闭）。
";

fn main() {
    let args = std::env::args().skip(1);
    let mut open: Option<PathBuf> = None;
    // `None` restores the persisted theme; a flag forces one.
    let mut mode: Option<Mode> = None;
    for arg in args {
        match arg.as_str() {
            "--light" => mode = Some(Mode::Light),
            "--dark" => mode = Some(Mode::Dark),
            "--selfcheck" => {
                #[cfg(feature = "selfcheck")]
                {
                    if let Err(message) = selfcheck::run() {
                        eprintln!("selfcheck failed: {message}");
                        std::process::exit(1);
                    }
                    return;
                }
                #[cfg(not(feature = "selfcheck"))]
                {
                    eprintln!(
                        "--selfcheck is not available in this build (enable the `selfcheck` feature)"
                    );
                    std::process::exit(2);
                }
            }
            "-h" | "--help" => {
                print!("{HELP}");
                return;
            }
            "-V" | "--version" => {
                println!("igui-editor {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            other if other.starts_with('-') => {
                eprintln!("unknown option: {other}\n\n{HELP}");
                std::process::exit(2);
            }
            path => open = Some(PathBuf::from(path)),
        }
    }
    host::run(open, mode);
}
