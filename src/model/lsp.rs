//! Language Server Protocol client and the small amount of state the editor
//! needs from it.
//!
//! This is a generic JSON-RPC client: a [`LanguageServerSpec`] decides which
//! executable to launch, which files it owns, and whether to use stdio or
//! loopback TCP. Rust, Go, TypeScript/JavaScript, C/C++, Python and Godot
//! GDScript are built in; the spec is declarative so a future external plugin
//! host can contribute another one without changing the editor.
//!
//! One session (child process) exists per `(spec, project root)`, so a monorepo
//! runs one server per language root and tab switches reuse it.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::model::doc::Document;
use crate::model::Workspace;
use crate::services::{LanguageServerSpec, ToolInstall};

const CHANGE_DEBOUNCE: Duration = Duration::from_millis(140);
const RETRY_AFTER: Duration = Duration::from_secs(5);
const MAX_COMPLETIONS: usize = 200;
const MAX_DIAGNOSTICS: usize = 500;
/// Public Godot class reference. The stable branch tracks the current release.
const GODOT_DOCS: &str = "https://docs.godotengine.org/en/stable/classes";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LspStatus {
    Off,
    Starting,
    Ready,
    Error(String),
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub start_line: usize,
    pub start_char: usize,
    pub end_line: usize,
    pub end_char: usize,
    /// LSP severity: 1 error, 2 warning, 3 information, 4 hint.
    pub severity: u32,
    pub message: String,
}

/// One LSP semantic token, in UTF-16 line/column coordinates.
#[derive(Clone, Debug)]
pub struct SemanticToken {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub kind: String,
}

#[derive(Clone, Debug)]
pub struct OutlineSymbol {
    pub name: String,
    pub kind: u32,
    pub depth: usize,
    pub line: u32,
    pub character: u32,
}

/// A Godot native symbol surfaced by the LSP hover declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeTarget {
    pub class: String,
    pub symbol: String,
    pub kind: NativeDocKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeDocKind {
    Class,
    Method,
    Property,
    Signal,
    Constant,
    Constructor,
    Operator,
}

impl NativeTarget {
    /// Official Godot online reference URL for this declaration.
    pub fn docs_url(&self) -> String {
        let class = self.class.trim_start_matches('@').to_ascii_lowercase();
        let file = self.class.to_ascii_lowercase();
        let mut url = format!("{GODOT_DOCS}/class_{file}.html");
        let section = match self.kind {
            NativeDocKind::Class => None,
            NativeDocKind::Method => Some("method"),
            NativeDocKind::Property => Some("property"),
            NativeDocKind::Signal => Some("signal"),
            NativeDocKind::Constant => Some("constant"),
            NativeDocKind::Constructor => Some("constructor"),
            NativeDocKind::Operator => Some("operator"),
        };
        if let Some(section) = section {
            url.push_str(&format!(
                "#class-{}-{}-{}",
                class,
                section,
                self.symbol.to_ascii_lowercase()
            ));
        }
        url
    }
}

/// A Godot native class/member returned by `textDocument/nativeSymbol`.
#[derive(Clone, Debug, Default)]
pub struct NativeSymbol {
    pub name: String,
    pub native_class: String,
    pub detail: String,
    pub documentation: String,
    pub kind: u32,
    pub children: Vec<NativeSymbol>,
}

#[derive(Clone, Debug)]
pub struct HoverUi {
    pub text: String,
    /// Present when the hover is a Godot native class/member; lets the UI offer
    /// an official documentation link.
    pub native: Option<NativeTarget>,
}

#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub label: String,
    pub insert: String,
    pub detail: String,
    /// Original LSP item, sent back to `completionItem/resolve`.
    pub raw: Value,
}

#[derive(Clone, Debug)]
pub struct CompletionUi {
    pub items: Vec<CompletionItem>,
    pub selected: usize,
    /// Markdown/hover-like documentation for the selected item.
    pub preview: Option<String>,
    pub preview_pending: bool,
    /// Set when the resolved declaration names a native Godot symbol.
    pub native: Option<NativeTarget>,
}

#[derive(Clone, Debug)]
pub struct Location {
    pub path: PathBuf,
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Debug)]
enum PendingKind {
    Initialize,
    Hover,
    HoverDocs,
    Completion,
    CompletionResolve { index: usize },
    Definition,
    NativeSymbol { class: String, symbol: String },
    Semantic(PathBuf),
    Symbols(PathBuf),
}

struct OpenDoc {
    version: i64,
    rev: u64,
    changed_at: Option<Instant>,
}

enum ClientEvent {
    Message(Value),
    ReadError(String),
}

struct LspClient {
    child: Child,
    outgoing: Sender<Value>,
    incoming: Receiver<ClientEvent>,
    next_id: u64,
}

impl LspClient {
    fn spawn(spec: &LanguageServerSpec, root: &Path) -> io::Result<Self> {
        let command = spec
            .command_env
            .as_deref()
            .and_then(|name| std::env::var(name).ok())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| spec.command.to_string());
        let mut process = Command::new(resolve_command(
            &command,
            root,
            workspace_binaries_enabled(),
        ));
        process
            .args(&spec.args)
            .current_dir(root)
            .stderr(Stdio::null());
        match spec.transport {
            crate::services::LanguageServerTransport::Stdio => Self::spawn_stdio(process),
            crate::services::LanguageServerTransport::Tcp => {
                let port = match spec.port {
                    Some(port) => port,
                    None => free_loopback_port()?,
                };
                if let Some(arg) = spec.port_arg.as_deref().filter(|arg| !arg.is_empty()) {
                    process.arg(arg).arg(port.to_string());
                } else if spec.port.is_none() {
                    return Err(io::Error::other(
                        "TCP language server needs a port_arg or a fixed port",
                    ));
                }
                process.stdin(Stdio::null()).stdout(Stdio::null());
                let child = process.spawn()?;
                Self::spawn_tcp(child, port)
            }
        }
    }

    fn spawn_stdio(mut process: Command) -> io::Result<Self> {
        process.stdin(Stdio::piped()).stdout(Stdio::piped());
        let mut child = process.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("language server stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("language server stdout unavailable"))?;

        let (outgoing, out_rx) = mpsc::channel::<Value>();
        thread::Builder::new()
            .name("igui-lsp-writer".into())
            .spawn(move || {
                let mut stdin = stdin;
                while let Ok(message) = out_rx.recv() {
                    if write_message(&mut stdin, &message).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| io::Error::other(format!("LSP writer thread: {error}")))?;

        let (events, incoming) = mpsc::channel::<ClientEvent>();
        thread::Builder::new()
            .name("igui-lsp-reader".into())
            .spawn(move || read_events(stdout, events))
            .map_err(|error| io::Error::other(format!("LSP reader thread: {error}")))?;

        Ok(Self {
            child,
            outgoing,
            incoming,
            next_id: 1,
        })
    }

    fn spawn_tcp(child: Child, port: u16) -> io::Result<Self> {
        let (outgoing, out_rx) = mpsc::channel::<Value>();
        let (events, incoming) = mpsc::channel::<ClientEvent>();
        let connect_events = events.clone();
        thread::Builder::new()
            .name("igui-lsp-connect".into())
            .spawn(move || {
                let stream = match connect_loopback(port, Duration::from_secs(8)) {
                    Ok(stream) => stream,
                    Err(error) => {
                        let _ = connect_events.send(ClientEvent::ReadError(format!(
                            "language server TCP {port}: {error}"
                        )));
                        return;
                    }
                };
                let read_stream = match stream.try_clone() {
                    Ok(stream) => stream,
                    Err(error) => {
                        let _ = connect_events.send(ClientEvent::ReadError(format!(
                            "language server TCP clone: {error}"
                        )));
                        return;
                    }
                };
                let writer_events = connect_events.clone();
                let writer =
                    thread::Builder::new()
                        .name("igui-lsp-writer".into())
                        .spawn(move || {
                            let mut stream = stream;
                            while let Ok(message) = out_rx.recv() {
                                if let Err(error) = write_message(&mut stream, &message) {
                                    let _ = writer_events.send(ClientEvent::ReadError(format!(
                                        "language server TCP write: {error}"
                                    )));
                                    break;
                                }
                            }
                        });
                if let Err(error) = writer {
                    let _ = connect_events.send(ClientEvent::ReadError(format!(
                        "LSP writer thread: {error}"
                    )));
                    return;
                }
                let reader_events = connect_events.clone();
                let reader = thread::Builder::new()
                    .name("igui-lsp-reader".into())
                    .spawn(move || read_events(read_stream, connect_events));
                if let Err(error) = reader {
                    let _ = reader_events.send(ClientEvent::ReadError(format!(
                        "LSP reader thread: {error}"
                    )));
                }
            })
            .map_err(|error| io::Error::other(format!("LSP connect thread: {error}")))?;

        Ok(Self {
            child,
            outgoing,
            incoming,
            next_id: 1,
        })
    }

    fn request(&mut self, method: &str, params: Value) -> Option<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.outgoing.send(message).ok()?;
        Some(id)
    }

    fn notify(&self, method: &str, params: Value) {
        let _ = self.outgoing.send(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }));
    }

    fn respond(&self, id: Value, result: Value) {
        let _ = self.outgoing.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }));
    }

    fn try_recv(&self) -> Option<ClientEvent> {
        self.incoming.try_recv().ok()
    }
}

fn read_events(stream: impl io::Read, events: Sender<ClientEvent>) {
    let mut reader = BufReader::new(stream);
    loop {
        match read_message(&mut reader) {
            Ok(Some(message)) => {
                if events.send(ClientEvent::Message(message)).is_err() {
                    break;
                }
            }
            Ok(None) => {
                let _ = events.send(ClientEvent::ReadError(
                    "language server closed its connection".into(),
                ));
                break;
            }
            Err(error) => {
                let _ = events.send(ClientEvent::ReadError(error.to_string()));
                break;
            }
        }
    }
}

fn free_loopback_port() -> io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

fn connect_loopback(port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + timeout;
    loop {
        match TcpStream::connect_timeout(&address, Duration::from_millis(200)) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                return Ok(stream);
            }
            Err(_) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(30));
            }
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!("could not connect to {address}: {error}"),
                ));
            }
        }
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// LSP framing: `Content-Length: N\r\n\r\n<N bytes>`.
fn write_message(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message)
        .map_err(|error| io::Error::other(format!("LSP JSON encode: {error}")))?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let length = length.ok_or_else(|| io::Error::other("LSP message without Content-Length"))?;
    if length > 64 * 1024 * 1024 {
        return Err(io::Error::other("LSP message too large"));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let value = serde_json::from_slice(&body)
        .map_err(|error| io::Error::other(format!("LSP JSON decode: {error}")))?;
    Ok(Some(value))
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let keep = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/');
        if keep {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    index += 3;
                    continue;
                }
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `file://` URI for an absolute path (percent-encoded, `/` kept).
pub fn path_to_uri(path: &Path) -> String {
    let mut text = path.to_string_lossy().replace('\\', "/");
    if !text.starts_with('/') {
        text.insert(0, '/');
    }
    if text.starts_with('/') {
        format!("file://{}", percent_encode(&text))
    } else {
        format!("file:///{}", percent_encode(&text))
    }
}

/// Parses a `file:` URI and normalizes it. Other URI schemes are ignored.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let decoded = percent_decode(rest);
    let path = if decoded.starts_with('/') {
        PathBuf::from(decoded)
    } else {
        PathBuf::from(format!("/{decoded}"))
    };
    Some(std::fs::canonicalize(&path).unwrap_or(path))
}

/// UTF-16 LSP position of a byte offset (LSP columns are UTF-16 code units).
pub fn position_of_byte(doc: &Document, byte: usize) -> (u32, u32) {
    let line = doc.line_of(byte);
    let start = doc.line_start(line);
    let column = doc.line(line)[..byte.saturating_sub(start).min(doc.line(line).len())]
        .encode_utf16()
        .count();
    (line as u32, column as u32)
}

/// Byte offset for an LSP line/UTF-16 column, clamped to the line.
pub fn byte_for_position(doc: &Document, line: u32, character: u32) -> usize {
    let line = line as usize;
    if line >= doc.line_count() {
        return doc.text().len();
    }
    let start = doc.line_start(line);
    let text = doc.line(line);
    let mut units = 0u32;
    for (offset, ch) in text.char_indices() {
        if units >= character {
            return start + offset;
        }
        units += ch.len_utf16() as u32;
    }
    start + text.len()
}

/// GUI-launched apps often have a minimal `PATH`; look in the usual toolchain
/// locations before giving up. Absolute/relative paths and commands with a
/// slash are left untouched.
fn resolve_command(command: &str, root: &Path, allow_workspace: bool) -> std::ffi::OsString {
    if command.contains('/') {
        return command.into();
    }
    let mut directories: Vec<PathBuf> = Vec::new();
    // A repository can ship its own `.bin` shims. Never execute those unless
    // the user explicitly trusts the workspace; PATH and user/global toolchains
    // below remain available in either case.
    if allow_workspace {
        directories.push(root.join("node_modules").join(".bin"));
    }
    directories.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        directories.push(home.join(".cargo/bin"));
        directories.push(home.join(".local/bin"));
        directories.push(home.join("go/bin"));
        directories.push(home.join(".local/share/pnpm"));
        directories.push(home.join("Library/pnpm"));
        directories.push(home.join(".npm-global/bin"));
        directories.push(home.join(".bun/bin"));
    }
    directories.push(PathBuf::from("/opt/homebrew/bin"));
    directories.push(PathBuf::from("/opt/homebrew/opt/rustup/bin"));
    directories.push(PathBuf::from("/opt/homebrew/opt/llvm/bin"));
    directories.push(PathBuf::from("/usr/local/bin"));
    directories.push(PathBuf::from("/usr/local/opt/rustup/bin"));
    directories.push(PathBuf::from(
        "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin",
    ));
    directories.push(PathBuf::from("/Library/Developer/CommandLineTools/usr/bin"));
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        let toolchains = home.join(".rustup/toolchains");
        if let Ok(entries) = std::fs::read_dir(&toolchains) {
            let mut bins: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path().join("bin"))
                .collect();
            bins.sort();
            directories.extend(bins);
        }
    }
    for directory in directories {
        let candidate = directory.join(command);
        if candidate.is_file() {
            return candidate.into_os_string();
        }
    }
    command.into()
}

/// Whether repository-local executable shims may be used.
///
/// This is deliberately opt-in: opening an untrusted project must not run a
/// `node_modules/.bin` file just because its language id matches.
fn workspace_binaries_enabled() -> bool {
    std::env::var("IGUI_EDITOR_TRUST_WORKSPACE")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
}

/// Root for a language server.
///
/// 1. the outermost workspace marker (`go.work`, `pnpm-workspace.yaml`,
///    `Cargo.lock`, …); markers are searched above the open folder too, so a
///    single file opened from a monorepo still finds its workspace root,
/// 2. otherwise the nearest project marker (`go.mod`, `Cargo.toml`, …),
/// 3. otherwise, for loose-file servers, the open folder or the file's parent.
fn project_root(path: &Path, spec: &LanguageServerSpec, bound: Option<&Path>) -> Option<PathBuf> {
    let start = if path.is_dir() { path } else { path.parent()? };
    let start = std::fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let inside_bound = bound.is_some_and(|bound| start.starts_with(bound));
    let mut nearest = None;
    let mut workspace = None;
    for ancestor in start.ancestors() {
        if nearest.is_none()
            && spec
                .project_markers
                .iter()
                .any(|marker| ancestor.join(marker).exists())
        {
            nearest = Some(ancestor.to_path_buf());
        }
        if spec
            .workspace_markers
            .iter()
            .any(|marker| ancestor.join(marker).exists())
        {
            workspace = Some(ancestor.to_path_buf());
        }
    }
    let root = workspace.or(nearest).or_else(|| {
        if !spec.loose_files {
            return None;
        }
        Some(match (inside_bound, bound) {
            (true, Some(bound)) => bound.to_path_buf(),
            _ => start.clone(),
        })
    })?;
    Some(std::fs::canonicalize(&root).unwrap_or(root))
}

fn severity_color(severity: u32, p: &crate::theme::Palette) -> igui_core::Color {
    match severity {
        1 => p.error,
        2 => p.syntax.number,
        3 => p.syntax.type_name,
        _ => p.faint,
    }
}

/// A missing language-server executable reported to the UI.
///
/// The editor never installs tools; the prompt only offers a copyable command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LspIssue {
    /// Executable name, e.g. `gopls`.
    pub tool: String,
    /// Human language name, e.g. `Go`.
    pub language: String,
    /// Copy-only install command; empty when the spec has no hint.
    pub command: String,
    pub note: Option<String>,
    /// Command the editor tried to start.
    pub source: String,
    /// Project root the server was needed for.
    pub root: PathBuf,
}

impl LspIssue {
    /// Body line for the tooling prompt.
    pub fn detail(&self) -> String {
        format!(
            "{} language features need `{}`. Searched PATH, node_modules/.bin and the usual toolchain folders while starting in {}.",
            self.language,
            self.source,
            self.root.display()
        )
    }
}

/// One language-server process for a `(spec, project root)` pair.
///
/// A monorepo keeps one session per language root; switching tabs reuses the
/// session instead of restarting the server.
struct Session {
    spec: LanguageServerSpec,
    root: PathBuf,
    client: Option<LspClient>,
    status: LspStatus,
    open_docs: HashMap<PathBuf, OpenDoc>,
    diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
    semantic_legend: Vec<String>,
    semantic_supported: bool,
    semantic: HashMap<PathBuf, Vec<SemanticToken>>,
    semantic_pending: HashSet<PathBuf>,
    semantic_in_flight: HashSet<PathBuf>,
    symbols: HashMap<PathBuf, Vec<OutlineSymbol>>,
    symbols_pending: HashSet<PathBuf>,
    symbols_in_flight: HashSet<PathBuf>,
    pending: HashMap<u64, (PendingKind, Instant)>,
    native_classes: HashSet<String>,
    native_symbols: HashMap<(String, String), NativeSymbol>,
    last_active: Instant,
    changed: bool,
}

impl Session {
    fn new(spec: LanguageServerSpec, root: PathBuf) -> Self {
        Self {
            spec,
            root,
            client: None,
            status: LspStatus::Off,
            open_docs: HashMap::new(),
            diagnostics: HashMap::new(),
            semantic_legend: Vec::new(),
            semantic_supported: false,
            semantic: HashMap::new(),
            semantic_pending: HashSet::new(),
            semantic_in_flight: HashSet::new(),
            symbols: HashMap::new(),
            symbols_pending: HashSet::new(),
            symbols_in_flight: HashSet::new(),
            pending: HashMap::new(),
            native_classes: HashSet::new(),
            native_symbols: HashMap::new(),
            last_active: Instant::now(),
            changed: false,
        }
    }

    fn reset_protocol(&mut self) {
        self.open_docs.clear();
        self.semantic_legend.clear();
        self.semantic_supported = false;
        self.semantic.clear();
        self.semantic_pending.clear();
        self.semantic_in_flight.clear();
        self.symbols.clear();
        self.symbols_pending.clear();
        self.symbols_in_flight.clear();
        self.pending.clear();
        self.native_classes.clear();
    }

    fn cancel_popup_requests(&mut self) {
        self.pending.retain(|_, (kind, _)| {
            !matches!(
                kind,
                PendingKind::Hover
                    | PendingKind::Completion
                    | PendingKind::CompletionResolve { .. }
            )
        });
    }

    fn respond_to_server(&self, id: Value, method: &str, params: &Value) {
        let Some(client) = self.client.as_ref() else {
            return;
        };
        let result = match method {
            "workspace/configuration" => {
                let count = params["items"].as_array().map_or(0, Vec::len);
                Value::Array(vec![Value::Null; count])
            }
            "workspace/workspaceFolders" => json!([{
                "uri": path_to_uri(&self.root),
                "name": self
                    .root
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("workspace"),
            }]),
            "window/showMessageRequest" => Value::Null,
            _ => Value::Null,
        };
        client.respond(id, result);
    }

    fn publish_diagnostics(&mut self, params: &Value) {
        let Some(uri) = params.get("uri").and_then(Value::as_str) else {
            return;
        };
        let Some(path) = uri_to_path(uri) else {
            return;
        };
        let mut diagnostics = Vec::new();
        if let Some(items) = params.get("diagnostics").and_then(Value::as_array) {
            for item in items.iter().take(MAX_DIAGNOSTICS) {
                let range = &item["range"];
                let start = &range["start"];
                let end = &range["end"];
                diagnostics.push(Diagnostic {
                    start_line: start["line"].as_u64().unwrap_or(0) as usize,
                    start_char: start["character"].as_u64().unwrap_or(0) as usize,
                    end_line: end["line"].as_u64().unwrap_or(0) as usize,
                    end_char: end["character"].as_u64().unwrap_or(0) as usize,
                    severity: item["severity"].as_u64().unwrap_or(1) as u32,
                    message: item["message"].as_str().unwrap_or("").to_string(),
                });
            }
        }
        diagnostics.sort_by_key(|diagnostic| {
            (
                diagnostic.start_line,
                diagnostic.start_char,
                diagnostic.severity,
            )
        });
        if diagnostics.is_empty() {
            self.diagnostics.remove(&path);
        } else {
            self.diagnostics.insert(path, diagnostics);
        }
        self.changed = true;
    }

    fn send_position(
        &mut self,
        method: &str,
        kind: PendingKind,
        path: &Path,
        doc: &Document,
        byte: usize,
    ) -> bool {
        if self.status != LspStatus::Ready {
            return false;
        }
        let Some(client) = self.client.as_mut() else {
            return false;
        };
        let (line, character) = position_of_byte(doc, byte.min(doc.text().len()));
        let id = client.request(
            method,
            json!({
                "textDocument": {"uri": path_to_uri(path)},
                "position": {"line": line, "character": character},
            }),
        );
        if let Some(id) = id {
            self.pending.insert(id, (kind, Instant::now()));
            true
        } else {
            false
        }
    }

    /// Synchronizes the documents this session owns (same spec, same root).
    fn sync(&mut self, owned: &[PathBuf], ws: &Workspace) {
        let live: HashSet<&Path> = owned.iter().map(PathBuf::as_path).collect();
        let closed: Vec<PathBuf> = self
            .open_docs
            .keys()
            .filter(|path| !live.contains(path.as_path()))
            .cloned()
            .collect();
        for path in closed {
            if let Some(client) = self.client.as_ref() {
                client.notify(
                    "textDocument/didClose",
                    json!({"textDocument": {"uri": path_to_uri(&path)}}),
                );
            }
            self.open_docs.remove(&path);
            self.diagnostics.remove(&path);
            self.semantic.remove(&path);
            self.semantic_pending.remove(&path);
            self.semantic_in_flight.remove(&path);
            self.symbols.remove(&path);
            self.symbols_pending.remove(&path);
            self.symbols_in_flight.remove(&path);
        }

        let now = Instant::now();
        let mut semantic_queue = Vec::new();
        let mut symbol_queue = Vec::new();
        for path in owned {
            let Some(doc) = ws
                .tabs
                .iter()
                .find(|doc| doc.path() == Some(path.as_path()))
            else {
                continue;
            };
            let uri = path_to_uri(path);
            match self.open_docs.get_mut(path) {
                None => {
                    if let Some(client) = self.client.as_ref() {
                        client.notify(
                            "textDocument/didOpen",
                            json!({
                                "textDocument": {
                                    "uri": uri,
                                    "languageId": self.spec.language_id(path),
                                    "version": 1,
                                    "text": doc.text(),
                                }
                            }),
                        );
                    }
                    self.open_docs.insert(
                        path.clone(),
                        OpenDoc {
                            version: 1,
                            rev: doc.rev(),
                            changed_at: None,
                        },
                    );
                    semantic_queue.push(path.clone());
                    symbol_queue.push(path.clone());
                }
                Some(open) => {
                    if open.rev == doc.rev() {
                        open.changed_at = None;
                        continue;
                    }
                    if open.changed_at.is_none() {
                        open.changed_at = Some(now);
                    }
                    if open
                        .changed_at
                        .is_some_and(|at| now.duration_since(at) >= CHANGE_DEBOUNCE)
                    {
                        open.version += 1;
                        open.rev = doc.rev();
                        open.changed_at = None;
                        if let Some(client) = self.client.as_ref() {
                            client.notify(
                                "textDocument/didChange",
                                json!({
                                    "textDocument": {"uri": uri, "version": open.version},
                                    "contentChanges": [{"text": doc.text()}],
                                }),
                            );
                        }
                        semantic_queue.push(path.clone());
                        symbol_queue.push(path.clone());
                    }
                }
            }
        }
        for path in semantic_queue {
            self.semantic_pending.insert(path);
        }
        for path in symbol_queue {
            self.symbols_pending.insert(path);
        }
        self.flush_semantic_requests();
        self.flush_symbol_requests();
    }

    fn flush_semantic_requests(&mut self) {
        if !self.semantic_supported {
            return;
        }
        let paths: Vec<PathBuf> = self
            .semantic_pending
            .iter()
            .filter(|path| !self.semantic_in_flight.contains(*path))
            .cloned()
            .collect();
        for path in paths {
            self.semantic_pending.remove(&path);
            let Some(client) = self.client.as_mut() else {
                return;
            };
            let request = client.request(
                "textDocument/semanticTokens/full",
                json!({"textDocument": {"uri": path_to_uri(&path)}}),
            );
            if let Some(id) = request {
                self.pending
                    .insert(id, (PendingKind::Semantic(path.clone()), Instant::now()));
                self.semantic_in_flight.insert(path);
            }
        }
    }

    fn flush_symbol_requests(&mut self) {
        let paths: Vec<PathBuf> = self
            .symbols_pending
            .iter()
            .filter(|path| !self.symbols_in_flight.contains(*path))
            .cloned()
            .collect();
        for path in paths {
            self.symbols_pending.remove(&path);
            let Some(client) = self.client.as_mut() else {
                return;
            };
            let request = client.request(
                "textDocument/documentSymbol",
                json!({"textDocument": {"uri": path_to_uri(&path)}}),
            );
            if let Some(id) = request {
                self.pending
                    .insert(id, (PendingKind::Symbols(path.clone()), Instant::now()));
                self.symbols_in_flight.insert(path);
            }
        }
    }

    fn cache_native_classes(&mut self, params: &Value) {
        let Some(items) = params.get("native_classes").and_then(Value::as_array) else {
            return;
        };
        self.native_classes.clear();
        for item in items {
            if let Some(name) = item.get("name").and_then(Value::as_str) {
                self.native_classes.insert(name.to_string());
            }
        }
    }

    /// Removes timed-out requests; returns the kinds the controller still has to
    /// resolve for the active session.
    fn expire_pending(&mut self) -> Vec<PendingKind> {
        let expired: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, (_, sent))| sent.elapsed() >= Duration::from_secs(15))
            .map(|(id, _)| *id)
            .collect();
        let mut out = Vec::new();
        for id in expired {
            match self.pending.remove(&id).map(|(kind, _)| kind) {
                Some(PendingKind::Semantic(path)) => {
                    self.semantic_in_flight.remove(&path);
                }
                Some(PendingKind::Symbols(path)) => {
                    self.symbols_in_flight.remove(&path);
                }
                Some(kind) => out.push(kind),
                None => {}
            }
        }
        out
    }

    fn diagnostics_for(&self, path: &Path) -> &[Diagnostic] {
        self.diagnostics.get(path).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Cache of `(spec, file) → project root`. Root resolution stats marker files,
/// so a frame must not walk the tree for every open tab.
#[derive(Default)]
struct RootCache {
    entries: HashMap<(String, PathBuf), Option<PathBuf>>,
    bound: Option<PathBuf>,
}

impl RootCache {
    fn sync_bound(&mut self, ws: &Workspace) {
        let bound = ws
            .folder()
            .map(|folder| std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf()));
        if self.bound != bound {
            self.bound = bound;
            self.entries.clear();
        }
    }

    fn forget(&mut self, path: &Path) {
        self.entries.retain(|(_, candidate), _| candidate != path);
    }

    /// Drops entries for files that are no longer open, so a long session with
    /// many opened files cannot grow the cache without bound.
    fn retain(&mut self, live: &HashSet<&Path>) {
        self.entries
            .retain(|(_, path), _| live.contains(path.as_path()));
    }

    fn root(&mut self, spec: &LanguageServerSpec, path: &Path) -> Option<PathBuf> {
        let key = (spec.id.clone(), path.to_path_buf());
        if let Some(root) = self.entries.get(&key) {
            return root.clone();
        }
        let root = project_root(path, spec, self.bound.as_deref());
        self.entries.insert(key, root.clone());
        root
    }
}

/// The application-side LSP state: one server per `(language, project root)`,
/// open-document synchronization, diagnostics and the latest hover/completion
/// popup. Transient UI belongs to the active session and is cleared on switch.
pub struct LspController {
    sessions: Vec<Session>,
    active: Option<usize>,
    specs: Vec<LanguageServerSpec>,
    roots: RootCache,
    last_active_path: Option<PathBuf>,
    failed: HashMap<(String, PathBuf), Instant>,
    notified: HashSet<String>,
    issue: Option<LspIssue>,
    enabled: bool,
    changed: bool,
    pub hover: Option<HoverUi>,
    pub hover_anchor: Option<igui_core::Vec2>,
    /// Last painted hover card, used to keep it alive while the pointer enters it.
    pub hover_rect: igui_core::Rect,
    /// Hit rectangle for the hover card's "Open Docs" action.
    pub hover_doc_hit: igui_core::Rect,
    /// Hit rectangle for the completion preview's docs action.
    pub completion_doc_hit: igui_core::Rect,
    pub completion: Option<CompletionUi>,
    completion_prefix: String,
    native_docs_target: Option<NativeTarget>,
    location: Option<Location>,
    native_result: Option<(String, String, NativeSymbol)>,
}

/// Live servers kept at once (one per language root). Older idle sessions are
/// dropped and restarted on demand.
const MAX_SESSIONS: usize = 4;
const IDLE_TIMEOUT: Duration = Duration::from_secs(600);

impl Default for LspController {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            active: None,
            specs: Vec::new(),
            roots: RootCache::default(),
            last_active_path: None,
            failed: HashMap::new(),
            notified: HashSet::new(),
            issue: None,
            enabled: true,
            changed: false,
            hover: None,
            hover_anchor: None,
            hover_rect: igui_core::Rect::ZERO,
            hover_doc_hit: igui_core::Rect::ZERO,
            completion_doc_hit: igui_core::Rect::ZERO,
            completion: None,
            completion_prefix: String::new(),
            native_docs_target: None,
            location: None,
            native_result: None,
        }
    }
}

impl LspController {
    pub fn status(&self) -> &LspStatus {
        static OFF: LspStatus = LspStatus::Off;
        self.active_session()
            .map(|session| &session.status)
            .unwrap_or(&OFF)
    }

    /// Whether language servers are enabled (settings/footer toggle).
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Applies the persisted on/off switch. Disabling kills every child.
    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        if !enabled {
            self.stop_all();
        }
        self.changed = true;
    }

    /// A missing-tool prompt waiting for the UI.
    pub fn take_issue(&mut self) -> Option<LspIssue> {
        self.issue.take()
    }

    /// Clears the retry back-off (tooling prompt `RETRY`).
    pub fn retry(&mut self) {
        self.failed.clear();
        self.notified.clear();
        self.issue = None;
        for session in &mut self.sessions {
            if session.client.is_none() {
                session.status = LspStatus::Off;
            }
        }
        self.changed = true;
    }

    fn stop_all(&mut self) {
        self.sessions.clear();
        self.active = None;
        self.failed.clear();
        self.notified.clear();
        self.issue = None;
        self.clear_transient();
    }

    fn clear_transient(&mut self) {
        self.hover = None;
        self.hover_anchor = None;
        self.hover_rect = igui_core::Rect::ZERO;
        self.hover_doc_hit = igui_core::Rect::ZERO;
        self.completion_doc_hit = igui_core::Rect::ZERO;
        self.completion = None;
        self.completion_prefix.clear();
        self.native_docs_target = None;
        self.location = None;
        self.native_result = None;
        self.changed = true;
    }

    fn active_session(&self) -> Option<&Session> {
        self.active.and_then(|index| self.sessions.get(index))
    }

    fn active_session_mut(&mut self) -> Option<&mut Session> {
        self.active.and_then(|index| self.sessions.get_mut(index))
    }

    /// The session that owns `path` (diagnostics, semantic tokens, symbols).
    fn session_for_path(&self, path: &Path) -> Option<&Session> {
        self.sessions
            .iter()
            .find(|session| {
                session.diagnostics.contains_key(path)
                    || session.semantic.contains_key(path)
                    || session.symbols.contains_key(path)
                    || session.open_docs.contains_key(path)
            })
            .or_else(|| {
                self.sessions.iter().find(|session| {
                    session.spec.matches_path(path) && path.starts_with(&session.root)
                })
            })
            .or_else(|| {
                self.sessions
                    .iter()
                    .find(|session| session.spec.matches_path(path))
            })
    }

    /// Whether a popup/diagnostic/status changed since the last frame.
    pub fn take_changed(&mut self) -> bool {
        let mut changed = std::mem::take(&mut self.changed);
        for session in &mut self.sessions {
            changed |= std::mem::take(&mut session.changed);
        }
        changed
    }

    pub fn supports(&self, path: &Path) -> bool {
        self.enabled && self.specs.iter().any(|spec| spec.matches_path(path))
    }

    pub fn is_busy(&self) -> bool {
        self.enabled
            && self.sessions.iter().any(|session| {
                matches!(session.status, LspStatus::Starting)
                    || session
                        .open_docs
                        .values()
                        .any(|doc| doc.changed_at.is_some())
                    || !session.pending.is_empty()
            })
    }

    /// Starts or reuses servers for the active document and synchronizes every
    /// open document each server owns. Safe to call every frame.
    pub fn tick(&mut self, ws: &Workspace, servers: &[LanguageServerSpec], enabled: bool) {
        self.set_enabled(enabled);
        if !self.enabled {
            return;
        }
        if self.specs != servers {
            self.specs = servers.to_vec();
        }
        self.roots.sync_bound(ws);

        let active_path = ws
            .active_doc()
            .and_then(|doc| doc.path())
            .map(Path::to_path_buf);
        if self.last_active_path != active_path {
            if let Some(path) = &active_path {
                self.roots.forget(path);
            }
            self.last_active_path = active_path.clone();
        }
        let live: HashSet<&Path> = ws.tabs.iter().filter_map(|doc| doc.path()).collect();
        self.roots.retain(&live);

        let desired = active_path.as_ref().and_then(|path| {
            self.specs.iter().find_map(|spec| {
                if !spec.matches_path(path) {
                    return None;
                }
                self.roots.root(spec, path).map(|root| (spec.clone(), root))
            })
        });
        let previous = self.active;
        // A docs/untitled tab has no path; keep the previous session active so
        // Godot documentation links can still request nativeSymbol.
        if let Some((spec, root)) = desired {
            let index = self
                .sessions
                .iter()
                .position(|session| session.spec.id == spec.id && session.root == root)
                .unwrap_or_else(|| {
                    self.sessions.push(Session::new(spec, root));
                    self.sessions.len() - 1
                });
            self.sessions[index].last_active = Instant::now();
            self.active = Some(index);
        }
        if self.active != previous {
            self.clear_transient();
        }

        if let Some(index) = self.active {
            if self.sessions[index].client.is_none() {
                self.start(index);
            }
        }

        for index in 0..self.sessions.len() {
            let expired = self.sessions[index].expire_pending();
            if self.active == Some(index) {
                self.apply_expired(expired);
            }
        }
        self.poll();
        for index in 0..self.sessions.len() {
            if self.sessions[index].status == LspStatus::Ready {
                self.sync_owned(index, ws);
            }
        }
        self.evict();
    }

    fn apply_expired(&mut self, expired: Vec<PendingKind>) {
        for kind in expired {
            match kind {
                PendingKind::CompletionResolve { index } => {
                    if let Some(completion) = self.completion.as_mut() {
                        if completion.selected == index {
                            completion.preview_pending = false;
                            completion.preview = Some("Documentation request timed out.".into());
                        }
                    }
                }
                PendingKind::NativeSymbol { class, symbol } => {
                    self.native_result = Some((
                        class.clone(),
                        symbol.clone(),
                        NativeSymbol {
                            name: symbol,
                            native_class: class,
                            detail: "Documentation unavailable".into(),
                            documentation:
                                "The language server did not return documentation for this symbol."
                                    .into(),
                            kind: 0,
                            children: Vec::new(),
                        },
                    ));
                }
                _ => {}
            }
        }
    }

    fn start(&mut self, index: usize) {
        let spec = self.sessions[index].spec.clone();
        let root = self.sessions[index].root.clone();
        let key = (spec.id.clone(), root.clone());
        if self
            .failed
            .get(&key)
            .is_some_and(|at| at.elapsed() < RETRY_AFTER)
        {
            return;
        }
        match LspClient::spawn(&spec, &root) {
            Ok(mut client) => {
                let params = initialize_params(&root, &spec);
                if let Some(id) = client.request("initialize", params) {
                    self.sessions[index]
                        .pending
                        .insert(id, (PendingKind::Initialize, Instant::now()));
                }
                self.sessions[index].client = Some(client);
                self.sessions[index].status = LspStatus::Starting;
                self.failed.remove(&key);
            }
            Err(error) => {
                self.sessions[index].status =
                    LspStatus::Error(format!("{}: {error}", spec.command));
                self.failed.insert(key, Instant::now());
                let missing = error.kind() == io::ErrorKind::NotFound;
                if missing && self.notified.insert(spec.id.clone()) {
                    let install = spec.install.clone().unwrap_or_else(|| ToolInstall {
                        tool: spec.command.clone(),
                        language: spec.language.clone(),
                        command: String::new(),
                        note: None,
                    });
                    self.issue = Some(LspIssue {
                        tool: install.tool,
                        language: install.language,
                        command: install.command,
                        note: install.note,
                        source: spec.command.clone(),
                        root,
                    });
                }
            }
        }
        self.changed = true;
    }

    fn sync_owned(&mut self, index: usize, ws: &Workspace) {
        let spec = self.sessions[index].spec.clone();
        let root = self.sessions[index].root.clone();
        let mut owned = Vec::new();
        for doc in &ws.tabs {
            let Some(path) = doc.path() else {
                continue;
            };
            if !spec.matches_path(path) {
                continue;
            }
            if self.roots.root(&spec, path).as_deref() != Some(root.as_path()) {
                continue;
            }
            owned.push(path.to_path_buf());
        }
        self.sessions[index].sync(&owned, ws);
    }

    fn evict(&mut self) {
        let now = Instant::now();
        let active = self.active;
        let mut keep: Vec<bool> = self
            .sessions
            .iter()
            .enumerate()
            .map(|(index, session)| {
                Some(index) == active || now.duration_since(session.last_active) <= IDLE_TIMEOUT
            })
            .collect();
        while keep.iter().filter(|keep| **keep).count() > MAX_SESSIONS {
            let victim = self
                .sessions
                .iter()
                .enumerate()
                .filter(|(index, _)| Some(*index) != active && keep[*index])
                .min_by_key(|(_, session)| session.last_active)
                .map(|(index, _)| index);
            let Some(victim) = victim else {
                break;
            };
            keep[victim] = false;
        }
        let mut index = 0;
        while index < self.sessions.len() {
            if keep[index] {
                index += 1;
                continue;
            }
            self.sessions.remove(index);
            if let Some(active) = self.active {
                if active > index {
                    self.active = Some(active - 1);
                } else if active == index {
                    self.active = None;
                }
            }
        }
    }

    fn poll(&mut self) {
        for index in 0..self.sessions.len() {
            let mut events = Vec::new();
            if let Some(client) = self.sessions[index].client.as_ref() {
                while let Some(event) = client.try_recv() {
                    events.push(event);
                }
            }
            for event in events {
                match event {
                    ClientEvent::Message(message) => self.handle_message(index, message),
                    ClientEvent::ReadError(error) => {
                        let root = self.sessions[index].root.clone();
                        let id = self.sessions[index].spec.id.clone();
                        self.failed.insert((id, root), Instant::now());
                        self.sessions[index].status = LspStatus::Error(error);
                        self.sessions[index].client = None;
                        self.sessions[index].reset_protocol();
                        self.sessions[index].changed = true;
                    }
                }
            }
        }
    }

    fn handle_message(&mut self, index: usize, message: Value) {
        let is_active = self.active == Some(index);
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(method) = method {
            if let Some(id) = message.get("id").cloned() {
                self.sessions[index].respond_to_server(id, &method, &message["params"]);
            } else if method == "textDocument/publishDiagnostics" {
                self.sessions[index].publish_diagnostics(&message["params"]);
            } else if method == "gdscript/capabilities" {
                self.sessions[index].cache_native_classes(&message["params"]);
            }
            return;
        }
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return;
        };
        let Some((kind, _)) = self.sessions[index].pending.remove(&id) else {
            return;
        };
        let result = message.get("result").cloned().unwrap_or(Value::Null);
        let error = message.get("error").cloned();
        match kind {
            PendingKind::Initialize => {
                if let Some(error) = error {
                    self.sessions[index].status = LspStatus::Error(format!("initialize: {error}"));
                    let root = self.sessions[index].root.clone();
                    let id = self.sessions[index].spec.id.clone();
                    self.failed.insert((id, root), Instant::now());
                    self.sessions[index].client = None;
                    self.sessions[index].reset_protocol();
                } else {
                    self.sessions[index].status = LspStatus::Ready;
                    let (supported, legend) = semantic_capability(&result);
                    self.sessions[index].semantic_supported = supported;
                    self.sessions[index].semantic_legend = legend;
                    if let Some(client) = self.sessions[index].client.as_ref() {
                        client.notify("initialized", json!({}));
                    }
                }
                self.sessions[index].changed = true;
            }
            PendingKind::Hover => {
                if is_active {
                    self.hover = hover_ui(&result);
                    self.changed = true;
                }
            }
            PendingKind::HoverDocs => {
                if is_active {
                    self.native_docs_target = hover_ui(&result).and_then(|hover| hover.native);
                    self.changed = true;
                }
            }
            PendingKind::Completion => {
                if is_active {
                    let items = parse_completions(&result, &self.completion_prefix);
                    self.completion = (!items.is_empty()).then_some(CompletionUi {
                        items,
                        selected: 0,
                        preview: None,
                        preview_pending: false,
                        native: None,
                    });
                    if self.completion.is_some() {
                        self.request_completion_resolve(0);
                    }
                    self.changed = true;
                }
            }
            PendingKind::CompletionResolve { index } => {
                if is_active {
                    if let Some(completion) = self.completion.as_mut() {
                        if completion.selected == index {
                            let text = completion_documentation(&result).or_else(|| {
                                completion.items.get(index).map(|item| item.detail.clone())
                            });
                            completion.preview = text;
                            completion.preview_pending = false;
                            completion.native = completion
                                .preview
                                .as_deref()
                                .and_then(native_target_from_hover);
                            self.changed = true;
                        }
                    }
                }
            }
            PendingKind::Definition => {
                if is_active {
                    self.location = parse_location(&result);
                    self.changed = true;
                }
            }
            PendingKind::NativeSymbol { class, symbol } => {
                if let Some(parsed) = parse_native_symbol(&result) {
                    self.sessions[index]
                        .native_symbols
                        .insert((class.clone(), symbol.clone()), parsed.clone());
                    // Documentation tabs are not files, so the request may be
                    // answered after the active document stopped being a path.
                    self.native_result = Some((class, symbol, parsed));
                    self.changed = true;
                }
            }
            PendingKind::Semantic(path) => {
                self.sessions[index].semantic_in_flight.remove(&path);
                if self.sessions[index].open_docs.contains_key(&path) {
                    let tokens =
                        parse_semantic_tokens(&result, &self.sessions[index].semantic_legend);
                    if tokens.is_empty() {
                        self.sessions[index].semantic.remove(&path);
                    } else {
                        self.sessions[index].semantic.insert(path, tokens);
                    }
                    self.sessions[index].changed = true;
                }
            }
            PendingKind::Symbols(path) => {
                self.sessions[index].symbols_in_flight.remove(&path);
                if self.sessions[index].open_docs.contains_key(&path) {
                    let symbols = parse_document_symbols(&result);
                    if symbols.is_empty() {
                        self.sessions[index].symbols.remove(&path);
                    } else {
                        self.sessions[index].symbols.insert(path, symbols);
                    }
                    self.sessions[index].changed = true;
                }
            }
        }
    }

    fn send_position(&mut self, method: &str, kind: PendingKind, doc: &Document) -> bool {
        self.send_position_at(method, kind, doc, doc.caret())
    }

    fn send_position_at(
        &mut self,
        method: &str,
        kind: PendingKind,
        doc: &Document,
        byte: usize,
    ) -> bool {
        let Some(path) = doc.path() else {
            return false;
        };
        let Some(session) = self.active_session_mut() else {
            return false;
        };
        if !session.spec.matches_path(path) {
            return false;
        }
        session.send_position(method, kind, path, doc, byte)
    }

    pub fn request_hover(&mut self, doc: &Document) -> bool {
        self.hover = None;
        self.hover_anchor = None;
        self.completion = None;
        if let Some(session) = self.active_session_mut() {
            session.cancel_popup_requests();
        }
        self.send_position("textDocument/hover", PendingKind::Hover, doc)
    }

    /// Hover at an arbitrary byte (mouse hover) with a viewport anchor.
    pub fn request_hover_at(
        &mut self,
        doc: &Document,
        byte: usize,
        anchor: igui_core::Vec2,
    ) -> bool {
        // Keep the previous card until the new response arrives; this avoids a
        // flicker while the pointer moves between identifiers.
        self.hover_anchor = Some(anchor);
        self.completion = None;
        if let Some(session) = self.active_session_mut() {
            session.cancel_popup_requests();
        }
        self.send_position_at("textDocument/hover", PendingKind::Hover, doc, byte)
    }

    /// Ctrl/Cmd-click documentation probe: ask for hover at a byte and let the
    /// caller open native docs if the declaration is a native symbol.
    pub fn request_native_docs_at(&mut self, doc: &Document, byte: usize) -> bool {
        self.native_docs_target = None;
        self.send_position_at("textDocument/hover", PendingKind::HoverDocs, doc, byte)
    }

    pub fn take_native_docs_target(&mut self) -> Option<NativeTarget> {
        self.native_docs_target.take()
    }

    pub fn request_completion(&mut self, doc: &Document) -> bool {
        self.completion = None;
        self.hover = None;
        if let Some(session) = self.active_session_mut() {
            session.cancel_popup_requests();
        }
        self.completion_prefix = completion_prefix(doc);
        self.send_position("textDocument/completion", PendingKind::Completion, doc)
    }

    pub fn request_definition(&mut self, doc: &Document) -> bool {
        self.hover = None;
        self.hover_anchor = None;
        self.completion = None;
        if let Some(session) = self.active_session_mut() {
            session.cancel_popup_requests();
        }
        self.send_position("textDocument/definition", PendingKind::Definition, doc)
    }

    /// Definition at an arbitrary byte (Ctrl/Cmd-click).
    pub fn request_definition_at(&mut self, doc: &Document, byte: usize) -> bool {
        self.hover = None;
        self.hover_anchor = None;
        self.completion = None;
        if let Some(session) = self.active_session_mut() {
            session.cancel_popup_requests();
        }
        self.send_position_at(
            "textDocument/definition",
            PendingKind::Definition,
            doc,
            byte,
        )
    }

    pub fn symbols_for(&self, path: &Path) -> &[OutlineSymbol] {
        self.session_for_path(path)
            .and_then(|session| session.symbols.get(path))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn semantic_for(&self, path: &Path) -> &[SemanticToken] {
        self.session_for_path(path)
            .and_then(|session| session.semantic.get(path))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn semantic_color(
        &self,
        kind: &str,
        p: &crate::theme::Palette,
    ) -> Option<igui_core::Color> {
        Some(match kind {
            "namespace" | "module" | "toolModule" => p.syntax.type_name.alpha_mul(0.78),
            "type" | "class" | "enum" | "interface" | "struct" | "typeParameter"
            | "builtinType" | "constParameter" | "typeAlias" | "union" => p.syntax.type_name,
            "parameter" => p.syntax.parameter,
            "property" | "field" | "enumMember" => p.syntax.property,
            "variable" | "event" => p.syntax.variable,
            "function" | "method" => p.syntax.function,
            "macro" | "builtinAttribute" | "attribute" | "derive" | "deriveHelper"
            | "macroBang" | "decorator" => p.syntax.macro_,
            "keyword" | "modifier" | "lifetime" | "selfKeyword" | "selfTypeKeyword" => {
                p.syntax.keyword
            }
            "comment" => p.syntax.comment,
            "string" | "escapeSequence" | "formatSpecifier" => p.syntax.string,
            "number" | "boolean" | "character" => p.syntax.number,
            "operator" => p.syntax.punctuation,
            "unresolvedReference" => p.error,
            "angle" | "arithmetic" | "bitwise" | "brace" | "colon" | "comma" | "comparison"
            | "dot" | "generic" | "logical" | "parenthesis" | "punctuation" => p.syntax.punctuation,
            _ => return None,
        })
    }

    pub fn take_location(&mut self) -> Option<Location> {
        self.location.take()
    }

    fn request_completion_resolve(&mut self, index: usize) {
        let Some(item) = self
            .completion
            .as_ref()
            .and_then(|completion| completion.items.get(index))
            .cloned()
        else {
            return;
        };
        let Some(session) = self.active_session_mut() else {
            return;
        };
        let Some(client) = session.client.as_mut() else {
            return;
        };
        let Some(id) = client.request("completionItem/resolve", item.raw) else {
            return;
        };
        session.pending.insert(
            id,
            (PendingKind::CompletionResolve { index }, Instant::now()),
        );
        if let Some(completion) = self.completion.as_mut() {
            completion.preview_pending = true;
            completion.preview = None;
            completion.native = None;
        }
    }

    pub fn move_completion(&mut self, delta: isize) {
        let Some(completion) = self.completion.as_mut() else {
            return;
        };
        if completion.items.is_empty() {
            return;
        }
        let len = completion.items.len() as isize;
        completion.selected = (completion.selected as isize + delta).rem_euclid(len) as usize;
        let selected = completion.selected;
        self.request_completion_resolve(selected);
    }

    pub fn take_completion(&mut self) -> Option<CompletionItem> {
        let completion = self.completion.take()?;
        completion.items.get(completion.selected).cloned()
    }

    pub fn dismiss_transient(&mut self) {
        self.hover = None;
        self.hover_anchor = None;
        self.hover_rect = igui_core::Rect::ZERO;
        self.hover_doc_hit = igui_core::Rect::ZERO;
        self.completion_doc_hit = igui_core::Rect::ZERO;
        self.completion = None;
        if let Some(session) = self.active_session_mut() {
            session.cancel_popup_requests();
        }
    }

    /// Whether `name` is one of Godot's native classes (e.g. `Node`).
    pub fn is_native_class(&self, name: &str) -> bool {
        self.active_session()
            .is_some_and(|session| session.native_classes.contains(name))
    }

    /// A cached native class/member document, if already fetched.
    pub fn native_symbol_cached(&self, class: &str, symbol: &str) -> Option<NativeSymbol> {
        self.active_session()
            .and_then(|session| {
                session
                    .native_symbols
                    .get(&(class.to_string(), symbol.to_string()))
            })
            .cloned()
    }

    /// Fetches a native class/member document through Godot's custom
    /// `textDocument/nativeSymbol` request.
    pub fn request_native_symbol(&mut self, class: &str, symbol: &str) -> bool {
        let Some(session) = self.active_session_mut() else {
            return false;
        };
        if session
            .native_symbols
            .contains_key(&(class.to_string(), symbol.to_string()))
        {
            return true;
        }
        if session.status != LspStatus::Ready {
            return false;
        }
        let pending = (
            PendingKind::NativeSymbol {
                class: class.to_string(),
                symbol: symbol.to_string(),
            },
            Instant::now(),
        );
        let Some(client) = session.client.as_mut() else {
            return false;
        };
        let request = client.request(
            "textDocument/nativeSymbol",
            json!({"native_class": class, "symbol_name": symbol}),
        );
        if let Some(id) = request {
            session.pending.insert(id, pending);
            true
        } else {
            false
        }
    }

    pub fn take_native_result(&mut self) -> Option<(String, String, NativeSymbol)> {
        self.native_result.take()
    }

    pub fn diagnostics_for(&self, path: &Path) -> &[Diagnostic] {
        self.session_for_path(path)
            .map(|session| session.diagnostics_for(path))
            .unwrap_or(&[])
    }

    pub fn diagnostic_color(&self, severity: u32, p: &crate::theme::Palette) -> igui_core::Color {
        severity_color(severity, p)
    }

    /// Highest-priority diagnostic severity for a path (1 = error).
    pub fn worst_severity(&self, path: &Path) -> Option<u32> {
        self.diagnostics_for(path)
            .iter()
            .map(|diagnostic| diagnostic.severity)
            .min()
    }

    /// Diagnostic whose range covers `byte`, if any. Used for hover cards.
    pub fn diagnostic_at(&self, doc: &Document, byte: usize) -> Option<Diagnostic> {
        let path = doc.path()?;
        self.diagnostics_for(path)
            .iter()
            .find(|diagnostic| {
                let start = byte_for_position(
                    doc,
                    diagnostic.start_line as u32,
                    diagnostic.start_char as u32,
                );
                let end =
                    byte_for_position(doc, diagnostic.end_line as u32, diagnostic.end_char as u32);
                if start == end {
                    byte >= start && byte <= end
                } else {
                    byte >= start && byte < end
                }
            })
            .cloned()
    }

    /// Footer summary for the active document. `None` hides the fact entirely.
    pub fn summary(&self, path: Option<&Path>) -> Option<String> {
        let path = path?;
        if !self.enabled {
            return Some("LSP OFF".into());
        }
        let session = self.session_for_path(path)?;
        if matches!(session.status, LspStatus::Error(_)) {
            return Some("LSP ERR".into());
        }
        if !matches!(session.status, LspStatus::Ready | LspStatus::Starting) {
            return None;
        }
        let diagnostics = session.diagnostics_for(path);
        if diagnostics.is_empty() {
            return Some("LSP".into());
        }
        let errors = diagnostics.iter().filter(|d| d.severity == 1).count();
        let warnings = diagnostics.iter().filter(|d| d.severity == 2).count();
        Some(format!("LSP {errors}E {warnings}W"))
    }
}

fn parse_native_symbol(result: &Value) -> Option<NativeSymbol> {
    fn parse(item: &Value, depth: usize) -> Option<NativeSymbol> {
        if depth > 8 || item.is_null() {
            return None;
        }
        let name = item.get("name").and_then(Value::as_str)?.to_string();
        let mut children = Vec::new();
        if let Some(items) = item.get("children").and_then(Value::as_array) {
            for child in items.iter().take(2_000) {
                if let Some(child) = parse(child, depth + 1) {
                    children.push(child);
                }
            }
        }
        Some(NativeSymbol {
            name,
            native_class: item
                .get("native_class")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            detail: item
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            documentation: item
                .get("documentation")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            kind: item.get("kind").and_then(Value::as_u64).unwrap_or(0) as u32,
            children,
        })
    }
    parse(result, 0)
}

fn parse_document_symbols(result: &Value) -> Vec<OutlineSymbol> {
    fn walk(items: &[Value], depth: usize, out: &mut Vec<OutlineSymbol>) {
        if depth > 12 || out.len() >= 2_000 {
            return;
        }
        for item in items {
            let Some(name) = item["name"].as_str().filter(|name| !name.is_empty()) else {
                continue;
            };
            let start = if item.get("selectionRange").is_some() {
                &item["selectionRange"]["start"]
            } else if item.get("location").is_some() {
                &item["location"]["range"]["start"]
            } else {
                &item["range"]["start"]
            };
            out.push(OutlineSymbol {
                name: name.to_string(),
                kind: item["kind"].as_u64().unwrap_or(13) as u32,
                depth,
                line: start["line"].as_u64().unwrap_or(0) as u32,
                character: start["character"].as_u64().unwrap_or(0) as u32,
            });
            if let Some(children) = item["children"].as_array() {
                walk(children, depth + 1, out);
            }
            if out.len() >= 2_000 {
                return;
            }
        }
    }
    let Some(items) = result.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk(items, 0, &mut out);
    out
}

fn semantic_capability(result: &Value) -> (bool, Vec<String>) {
    let provider = &result["capabilities"]["semanticTokensProvider"];
    let types = provider["legend"]["tokenTypes"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let full = provider
        .get("full")
        .is_some_and(|value| !value.is_null() && value.as_bool() != Some(false));
    (full && !types.is_empty(), types)
}

fn parse_semantic_tokens(result: &Value, legend: &[String]) -> Vec<SemanticToken> {
    let Some(data) = result["data"].as_array() else {
        return Vec::new();
    };
    let mut tokens = Vec::new();
    let mut line = 0usize;
    let mut start = 0usize;
    for chunk in data.chunks_exact(5) {
        let delta_line = chunk[0].as_u64().unwrap_or(0) as usize;
        let delta_start = chunk[1].as_u64().unwrap_or(0) as usize;
        let length = chunk[2].as_u64().unwrap_or(0) as usize;
        let kind_index = chunk[3].as_u64().unwrap_or(0) as usize;
        if delta_line > 0 {
            line += delta_line;
            start = delta_start;
        } else {
            start = start.saturating_add(delta_start);
        }
        let Some(kind) = legend.get(kind_index) else {
            continue;
        };
        if length > 0 {
            tokens.push(SemanticToken {
                line,
                start,
                end: start.saturating_add(length),
                kind: kind.clone(),
            });
        }
        if tokens.len() >= 200_000 {
            break;
        }
    }
    tokens
}

fn initialize_params(root: &Path, spec: &LanguageServerSpec) -> Value {
    json!({
        "processId": null,
        "clientInfo": {"name": "igui-editor", "version": env!("CARGO_PKG_VERSION")},
        "rootUri": path_to_uri(root),
        "rootPath": root.to_string_lossy(),
        "workspaceFolders": [{
            "uri": path_to_uri(root),
            "name": root.file_name().and_then(|name| name.to_str()).unwrap_or("workspace"),
        }],
        "capabilities": {
            "workspace": {"workspaceFolders": true, "configuration": true},
            "textDocument": {
                "synchronization": {
                    "dynamicRegistration": false,
                    "didSave": false,
                    "change": 1
                },
                "hover": {"contentFormat": ["markdown", "plaintext"]},
                "completion": {
                    "dynamicRegistration": false,
                    "completionItem": {
                        "snippetSupport": false,
                        "documentationFormat": ["markdown", "plaintext"]
                    }
                },
                "definition": {"dynamicRegistration": false, "linkSupport": false},
                "documentSymbol": {
                    "dynamicRegistration": false,
                    "hierarchicalDocumentSymbolSupport": true,
                    "labelSupport": false
                },
                "semanticTokens": {
                    "dynamicRegistration": false,
                    "requests": {"full": true, "range": false},
                    "tokenTypes": [
                        "namespace", "type", "class", "enum", "interface", "struct",
                        "typeParameter", "parameter", "variable", "property",
                        "enumMember", "event", "function", "method", "macro",
                        "keyword", "modifier", "comment", "string", "number",
                        "regexp", "operator", "decorator"
                    ],
                    "tokenModifiers": [
                        "declaration", "definition", "readonly", "static",
                        "deprecated", "abstract", "async", "modification",
                        "documentation", "defaultLibrary"
                    ],
                    "formats": ["relative"],
                    "overlappingTokenSupport": false,
                    "multilineTokenSupport": false
                },
                "publishDiagnostics": {"relatedInformation": false}
            }
        },
        "initializationOptions": {
            "language": spec.language,
        }
    })
}

fn hover_ui(result: &Value) -> Option<HoverUi> {
    let text = hover_text(result)?;
    let native = native_target_from_hover(&text);
    Some(HoverUi { text, native })
}

fn hover_text(result: &Value) -> Option<String> {
    if result.is_null() {
        return None;
    }
    let contents = result.get("contents").unwrap_or(result);
    let mut parts = Vec::new();
    match contents {
        Value::String(text) => parts.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                if let Some(text) = markdown_value(item) {
                    parts.push(text);
                }
            }
        }
        Value::Object(_) => {
            if let Some(text) = markdown_value(contents) {
                parts.push(text);
            }
        }
        _ => {}
    }
    let text = parts.join("\n\n");
    (!text.trim().is_empty()).then_some(text)
}

fn native_target_from_hover(text: &str) -> Option<NativeTarget> {
    let first = text.lines().next()?.trim();
    if let Some(rest) = first.strip_prefix("<Native> class ") {
        let class = rest.split_whitespace().next()?;
        return Some(NativeTarget {
            class: class.to_string(),
            symbol: class.to_string(),
            kind: NativeDocKind::Class,
        });
    }
    let (kind, rest) = first
        .strip_prefix("func ")
        .map(|rest| (NativeDocKind::Method, rest))
        .or_else(|| {
            first
                .strip_prefix("const ")
                .map(|rest| (NativeDocKind::Constant, rest))
        })
        .or_else(|| {
            first
                .strip_prefix("var ")
                .map(|rest| (NativeDocKind::Property, rest))
        })
        .or_else(|| {
            first
                .strip_prefix("signal ")
                .map(|rest| (NativeDocKind::Signal, rest))
        })?;
    let token = rest.split_whitespace().next()?;
    let (class, raw_symbol) = token.split_once('.')?;
    let symbol = raw_symbol
        .split(['(', ':', '=', ' '])
        .next()
        .unwrap_or(raw_symbol);
    if class.is_empty() || symbol.is_empty() {
        return None;
    }
    Some(NativeTarget {
        class: class.to_string(),
        symbol: symbol.to_string(),
        kind,
    })
}

fn completion_prefix(doc: &Document) -> String {
    let caret = doc.caret().min(doc.text().len());
    let line = doc.line_of(caret);
    let start = doc.line_start(line);
    let before = &doc.line(line)[..caret.saturating_sub(start).min(doc.line(line).len())];
    before
        .rsplit(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
        .next()
        .unwrap_or("")
        .chars()
        .take(64)
        .collect()
}

fn markdown_value(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    value
        .get("value")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn completion_documentation(result: &Value) -> Option<String> {
    let documentation = result.get("documentation")?;
    let text = match documentation {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(markdown_value)
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => markdown_value(documentation)?,
    };
    (!text.trim().is_empty()).then_some(text)
}

fn parse_completions(result: &Value, prefix: &str) -> Vec<CompletionItem> {
    let items = result
        .as_array()
        .or_else(|| result.get("items").and_then(Value::as_array));
    let Some(items) = items else {
        return Vec::new();
    };
    let prefix_lower = prefix.to_ascii_lowercase();
    let mut parsed: Vec<CompletionItem> = items
        .iter()
        .filter_map(|item| {
            let label = item["label"].as_str().unwrap_or("").to_string();
            if label.is_empty() {
                return None;
            }
            let insert = item["textEdit"]["newText"]
                .as_str()
                .or_else(|| item["insertText"].as_str())
                .unwrap_or(&label)
                .to_string();
            if !prefix.is_empty()
                && !insert.to_ascii_lowercase().starts_with(&prefix_lower)
                && !label.to_ascii_lowercase().starts_with(&prefix_lower)
            {
                return None;
            }
            let detail = item["detail"].as_str().unwrap_or("").to_string();
            Some(CompletionItem {
                label,
                insert,
                detail,
                raw: item.clone(),
            })
        })
        .collect();
    // Godot returns a large, unsorted list (locals, members, globals, types).
    // Filter by the prefix first, then put an exact insert before longer names.
    if !prefix.is_empty() {
        parsed.sort_by_key(|item| {
            let exact = item.insert.eq_ignore_ascii_case(prefix);
            let label_exact = item
                .label
                .trim_end_matches(['…', '(', ')'])
                .eq_ignore_ascii_case(prefix);
            (
                !(exact || label_exact),
                item.insert.len(),
                item.label.clone(),
            )
        });
    }
    let mut seen = HashSet::new();
    parsed.retain(|item| seen.insert((item.insert.clone(), item.label.clone())));
    parsed.truncate(MAX_COMPLETIONS);
    parsed
}

fn parse_location(result: &Value) -> Option<Location> {
    let first = if let Some(items) = result.as_array() {
        items.first()?
    } else {
        result
    };
    let uri = first["uri"]
        .as_str()
        .or_else(|| first["targetUri"].as_str())?;
    let range = if first.get("range").is_some() {
        &first["range"]
    } else {
        &first["targetSelectionRange"]
    };
    let start = &range["start"];
    Some(Location {
        path: uri_to_path(uri)?,
        line: start["line"].as_u64().unwrap_or(0) as u32,
        character: start["character"].as_u64().unwrap_or(0) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_binaries_require_explicit_trust() {
        let dir =
            std::env::temp_dir().join(format!("igui-editor-lsp-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bin = dir.join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin).unwrap();
        let command = format!("igui-editor-test-lsp-{}", std::process::id());
        let candidate = bin.join(&command);
        std::fs::write(&candidate, "#!/bin/sh\n").unwrap();

        let untrusted = resolve_command(&command, &dir, false);
        assert_ne!(Path::new(&untrusted), candidate.as_path());
        let trusted = resolve_command(&command, &dir, true);
        assert_eq!(Path::new(&trusted), candidate.as_path());
        let _ = std::fs::remove_dir_all(dir);
    }
    use std::io::Cursor;

    fn fixture_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("igui-lsp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn workspace_marker_wins_over_nested_package_marker() {
        let dir = fixture_dir("monorepo-go");
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::write(dir.join("go.work"), "go 1.22\n").unwrap();
        std::fs::write(dir.join("a/go.mod"), "module a\n").unwrap();
        std::fs::write(dir.join("b/go.mod"), "module b\n").unwrap();
        let spec = crate::services::go::go_server();
        let bound = std::fs::canonicalize(&dir).unwrap();
        let root = project_root(&dir.join("a/main.go"), &spec, Some(&bound)).unwrap();
        assert_eq!(root, std::fs::canonicalize(&dir).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nearest_package_marker_is_used_without_a_workspace_marker() {
        let dir = fixture_dir("monorepo-package");
        std::fs::create_dir_all(dir.join("a/src")).unwrap();
        std::fs::write(dir.join("a/go.mod"), "module a\n").unwrap();
        let spec = crate::services::go::go_server();
        let bound = std::fs::canonicalize(&dir).unwrap();
        let root = project_root(&dir.join("a/src/main.go"), &spec, Some(&bound)).unwrap();
        assert_eq!(root, std::fs::canonicalize(dir.join("a")).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_markers_are_found_above_the_open_folder() {
        let dir = fixture_dir("monorepo-bound");
        std::fs::create_dir_all(dir.join("packages/web/src")).unwrap();
        std::fs::write(dir.join("pnpm-workspace.yaml"), "packages: []\n").unwrap();
        std::fs::write(dir.join("packages/web/package.json"), "{}\n").unwrap();
        let spec = crate::services::typescript::typescript_server();
        // Opening a single file inside a package still finds the repository
        // workspace root, so one server serves every package.
        let bound = std::fs::canonicalize(dir.join("packages/web")).unwrap();
        let root = project_root(&dir.join("packages/web/src/a.ts"), &spec, Some(&bound)).unwrap();
        assert_eq!(root, std::fs::canonicalize(&dir).unwrap());
        let root = project_root(&dir.join("packages/web/src/a.ts"), &spec, None).unwrap();
        assert_eq!(root, std::fs::canonicalize(&dir).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loose_file_servers_fall_back_to_the_file_folder() {
        let dir = fixture_dir("loose");
        let spec = crate::services::python::python_server();
        let root = project_root(&dir.join("script.py"), &spec, None).unwrap();
        assert_eq!(root, std::fs::canonicalize(&dir).unwrap());
        // A marker-only server refuses to start outside a project.
        let rust = crate::services::lsp::rust_server();
        assert!(project_root(&dir.join("loose.rs"), &rust, None).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn language_ids_follow_the_session_spec() {
        let ts = crate::services::typescript::typescript_server();
        assert_eq!(ts.language_id(Path::new("/p/app.tsx")), "typescriptreact");
        let c = crate::services::clang::clang_server();
        assert_eq!(c.language_id(Path::new("/p/main.h")), "c");
        assert_eq!(c.language_id(Path::new("/p/main.cpp")), "cpp");
    }

    #[test]
    fn sessions_are_keyed_by_spec_and_root() {
        let dir = fixture_dir("sessions");
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::write(dir.join("a/go.mod"), "module a\n").unwrap();
        std::fs::write(dir.join("b/go.mod"), "module b\n").unwrap();
        let spec = crate::services::go::go_server();
        let mut lsp = LspController {
            specs: vec![spec.clone()],
            sessions: vec![
                Session::new(spec.clone(), dir.join("a")),
                Session::new(spec.clone(), dir.join("b")),
            ],
            active: Some(0),
            ..LspController::default()
        };
        lsp.sessions[0]
            .diagnostics
            .insert(dir.join("a/main.go"), Vec::new());
        lsp.sessions[1]
            .diagnostics
            .insert(dir.join("b/main.go"), Vec::new());
        assert!(lsp.enabled());
        assert!(lsp.supports(&dir.join("a/main.go")));
        assert_eq!(
            lsp.session_for_path(&dir.join("b/main.go"))
                .map(|session| session.root.as_path()),
            Some(dir.join("b").as_path())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabling_drops_every_session() {
        let spec = crate::services::go::go_server();
        let mut lsp = LspController::default();
        lsp.sessions.push(Session::new(spec, PathBuf::from("/tmp")));
        lsp.active = Some(0);
        lsp.set_enabled(false);
        assert!(!lsp.enabled());
        assert!(lsp.sessions.is_empty());
        assert_eq!(
            lsp.summary(Some(Path::new("/tmp/a.go"))).as_deref(),
            Some("LSP OFF")
        );
    }

    #[test]
    fn uri_round_trip_encodes_spaces() {
        let path = Path::new("/tmp/a b/ünicode.rs");
        let uri = path_to_uri(path);
        assert!(uri.starts_with("file:///tmp/a%20b/"));
        assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
        assert_eq!(uri_to_path("https://example.com/x"), None);
    }

    #[test]
    fn framing_round_trip_and_missing_length() {
        let message = json!({"jsonrpc": "2.0", "id": 3, "result": {"ok": true}});
        let mut encoded = Vec::new();
        write_message(&mut encoded, &message).unwrap();
        assert!(encoded.starts_with(b"Content-Length: "));
        let mut reader = Cursor::new(encoded);
        assert_eq!(read_message(&mut reader).unwrap(), Some(message));
        let mut empty = Cursor::new(b"X: 1\r\n\r\n".to_vec());
        assert!(read_message(&mut empty).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn tcp_transport_connects_and_exchanges_json_rpc() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let spec = LanguageServerSpec {
            id: "fake-tcp".into(),
            language: "gdscript".into(),
            extensions: vec!["gd".into()],
            project_markers: vec![],
            workspace_markers: vec![],
            language_ids: Default::default(),
            loose_files: false,
            command: "/bin/sh".into(),
            command_env: None,
            args: vec!["-c".into(), "sleep 30".into()],
            transport: crate::services::LanguageServerTransport::Tcp,
            port_arg: None,
            port: Some(port),
            install: None,
        };
        let mut client = LspClient::spawn(&spec, Path::new("/tmp")).unwrap();
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let id = client
            .request("initialize", json!({"rootUri": "file:///tmp"}))
            .unwrap();
        let request = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(request["method"], "initialize");
        assert_eq!(request["id"], id);
        write_message(
            &mut reader.get_mut(),
            &json!({"jsonrpc": "2.0", "id": id, "result": {"capabilities": {}}}),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match client.try_recv() {
                Some(ClientEvent::Message(message)) => {
                    assert_eq!(message["id"], id);
                    break;
                }
                Some(ClientEvent::ReadError(error)) => panic!("TCP client error: {error}"),
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                None => panic!("TCP client did not deliver the response"),
            }
        }
    }

    #[test]
    fn parses_relative_semantic_tokens() {
        let (supported, legend) = semantic_capability(&json!({
            "capabilities": {
                "semanticTokensProvider": {
                    "full": true,
                    "legend": {"tokenTypes": ["variable", "function"]}
                }
            }
        }));
        assert!(supported);
        let tokens =
            parse_semantic_tokens(&json!({"data": [0, 4, 2, 1, 0, 1, 4, 3, 0, 0]}), &legend);
        assert_eq!(tokens.len(), 2);
        assert_eq!((tokens[0].line, tokens[0].start, tokens[0].end), (0, 4, 6));
        assert_eq!(tokens[0].kind, "function");
        assert_eq!((tokens[1].line, tokens[1].start), (1, 4));
    }

    #[test]
    fn completion_filters_all_results_before_capping() {
        let mut items = (0..300)
            .map(|index| json!({"label": format!("filler_{index}"), "insertText": format!("filler_{index}")}))
            .collect::<Vec<_>>();
        items.push(json!({"label": "print_orphan_nodes()", "insertText": "print_orphan_nodes"}));
        items.push(json!({"label": "print(…)", "insertText": "print"}));
        let items = parse_completions(&json!({"items": items}), "pri");
        assert_eq!(
            items.first().map(|item| item.insert.as_str()),
            Some("print")
        );
        assert!(items.iter().all(|item| item.insert.starts_with("pri")));
    }

    #[test]
    fn parses_native_symbols_and_children() {
        let symbol = parse_native_symbol(&json!({
            "name": "Node",
            "native_class": "Node",
            "detail": "class Node extends Object",
            "documentation": "Use [method _ready].",
            "kind": 5,
            "children": [{
                "name": "_ready",
                "native_class": "Node",
                "detail": "func Node._ready() -> void",
                "documentation": "Ready.",
                "kind": 6
            }]
        }))
        .unwrap();
        assert_eq!(symbol.name, "Node");
        assert_eq!(symbol.children.len(), 1);
        assert_eq!(symbol.children[0].name, "_ready");
        assert_eq!(symbol.children[0].kind, 6);
    }

    #[test]
    fn native_capabilities_populate_the_class_index() {
        let mut lsp = LspController::default();
        lsp.sessions.push(Session::new(
            crate::services::godot::godot_server(),
            PathBuf::from("/tmp"),
        ));
        lsp.active = Some(0);
        lsp.sessions[0].cache_native_classes(&json!({
            "native_classes": [
                {"name": "Node", "inherits": "Object"},
                {"name": "Sprite2D", "inherits": "Node2D"}
            ]
        }));
        assert!(lsp.is_native_class("Node"));
        assert!(lsp.is_native_class("Sprite2D"));
        assert!(!lsp.is_native_class("print"));
    }

    #[test]
    fn native_hover_parses_class_and_global_function() {
        let node = native_target_from_hover("<Native> class Node extends Object\n\n...").unwrap();
        assert_eq!(node.class, "Node");
        assert_eq!(node.kind, NativeDocKind::Class);
        assert_eq!(
            node.docs_url(),
            "https://docs.godotengine.org/en/stable/classes/class_node.html"
        );
        let print =
            native_target_from_hover("func @GlobalScope.print(...) -> void\n\n...").unwrap();
        assert_eq!(print.class, "@GlobalScope");
        assert_eq!(print.symbol, "print");
        assert_eq!(
            print.docs_url(),
            "https://docs.godotengine.org/en/stable/classes/class_@globalscope.html#class-globalscope-method-print"
        );
        let ready = native_target_from_hover("func Node._ready() -> void\n\n...").unwrap();
        assert_eq!(
            (ready.class.as_str(), ready.symbol.as_str()),
            ("Node", "_ready")
        );
        assert_eq!(ready.kind, NativeDocKind::Method);
        let property = native_target_from_hover("var Node.name: StringName\n\n...").unwrap();
        assert_eq!(
            (property.class.as_str(), property.symbol.as_str()),
            ("Node", "name")
        );
        assert_eq!(property.kind, NativeDocKind::Property);
    }

    #[test]
    fn positions_use_utf16_columns() {
        let mut doc = Document::untitled(1, 1);
        doc.insert_text("a😀b");
        assert_eq!(position_of_byte(&doc, 0), (0, 0));
        assert_eq!(position_of_byte(&doc, 1), (0, 1));
        assert_eq!(position_of_byte(&doc, 5), (0, 3));
        assert_eq!(byte_for_position(&doc, 0, 3), 5);
        assert_eq!(byte_for_position(&doc, 0, 99), doc.text().len());
    }

    #[test]
    fn parses_hover_completion_and_locations() {
        assert_eq!(
            hover_text(&json!({"contents": {"kind": "markdown", "value": "`fn` main"}})),
            Some("`fn` main".into())
        );
        let items = parse_completions(
            &json!({"items": [
                {"label": "println!", "insertText": "println!", "detail": "macro"},
                {"label": "print", "textEdit": {"newText": "print"}}
            ]}),
            "pri",
        );
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].insert, "print");
        assert_eq!(items[1].insert, "println!");
        let location = parse_location(&json!([{
            "uri": "file:///tmp/main.rs",
            "range": {"start": {"line": 4, "character": 2}, "end": {"line": 4, "character": 5}}
        }]))
        .unwrap();
        assert_eq!(location.path, PathBuf::from("/tmp/main.rs"));
        assert_eq!((location.line, location.character), (4, 2));
    }
}
