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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use super::watch::{self, FileEvent, FileEventKind};

use serde_json::{json, Value};

use super::processes;
use crate::model::doc::Document;
use crate::model::Workspace;
use crate::services::{
    AttachTarget, FileChangePolicy, LanguageServerSpec, LanguageServerTransport, ToolInstall,
};

const CHANGE_DEBOUNCE: Duration = Duration::from_millis(140);
/// Wait before re-sending open documents to a server that rescans the project
/// itself. Godot's "rescan while unfocused" timer fires every 0.5 s.
const RESCAN_DELAY: Duration = Duration::from_millis(800);
/// Follow-up pass in case the first scan was still running (large projects).
const RESCAN_FOLLOWUP: Duration = Duration::from_millis(2_000);
/// Re-parse passes per structural change: the first plus two follow-ups.
const RESCAN_PASSES: u8 = 3;
/// Godot's project scan rewrites this file when the global class list changes.
const CLASS_CACHE_FILE: &str = "global_script_class_cache.cfg";
const RETRY_AFTER: Duration = Duration::from_secs(5);
/// Attached servers reconnect promptly: the editor may be opened after us.
const ATTACH_RETRY: Duration = Duration::from_secs(2);
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

/// Optional server features the editor can use, read from `initialize`.
///
/// The protocol is the same for every server, but support varies (Godot 4.7
/// gained signature help, references, occurrence highlights and rename, while
/// its formatting/code-action methods stay unimplemented). Requests are gated
/// on these flags so an older server gets a clear status line instead of a
/// `Method not found` round trip.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerCapabilities {
    /// `completionProvider.triggerCharacters`; `.` is always accepted.
    pub completion_triggers: Vec<char>,
    pub signature_help: bool,
    pub references: bool,
    pub document_highlight: bool,
    pub rename: bool,
    pub prepare_rename: bool,
}

/// One text edit from a `workspace/rename` response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceTextEdit {
    pub path: PathBuf,
    pub start_line: u32,
    pub start_char: u32,
    pub end_line: u32,
    pub end_char: u32,
    pub new_text: String,
}

/// The document the rename was requested for, used to reject stale edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameContext {
    pub doc_id: u64,
    pub rev: u64,
    pub path: PathBuf,
    pub caret: usize,
    pub old_name: String,
    /// Chosen in the prompt; empty while only preparing.
    pub new_name: String,
}

/// One LSP range in UTF-16 line/column coordinates (occurrence highlights).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextRange {
    pub start_line: u32,
    pub start_char: u32,
    pub end_line: u32,
    pub end_char: u32,
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

/// One candidate from `textDocument/signatureHelp`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    /// Display label (`func clamp(value: Variant, min: Variant, …)`).
    pub label: String,
    pub documentation: Option<String>,
    /// Parameter spans as byte offsets into [`label`], for the active highlight.
    pub parameters: Vec<(usize, usize)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureHelpUi {
    pub signatures: Vec<Signature>,
    pub active_signature: usize,
    pub active_parameter: usize,
}

impl SignatureHelpUi {
    pub fn active(&self) -> Option<&Signature> {
        self.signatures.get(
            self.active_signature
                .min(self.signatures.len().saturating_sub(1)),
        )
    }

    /// Byte span of the active parameter in the active signature's label.
    pub fn active_span(&self) -> Option<(usize, usize)> {
        let signature = self.active()?;
        if signature.parameters.is_empty() {
            return None;
        }
        let index = self
            .active_parameter
            .min(signature.parameters.len().saturating_sub(1));
        signature.parameters.get(index).copied()
    }
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
    SignatureHelp,
    Highlights(PathBuf),
    References { symbol: String },
    PrepareRename,
    Rename,
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
    /// `None` when attached to a server we did not start (the running Godot
    /// editor); dropping the client must not kill the user's editor.
    child: Option<Child>,
    outgoing: Sender<Value>,
    incoming: Receiver<ClientEvent>,
    next_id: u64,
}

impl LspClient {
    fn spawn(spec: &LanguageServerSpec, root: &Path, allow_workspace: bool) -> io::Result<Self> {
        let command = spec
            .command_env
            .as_deref()
            .and_then(|name| std::env::var(name).ok())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| spec.command.to_string());
        let resolved = resolve_command(&command, root, allow_workspace);
        let mut process = Command::new(&resolved);
        process
            .args(&spec.args)
            .current_dir(root)
            .stderr(Stdio::null());
        match spec.transport {
            LanguageServerTransport::Stdio => Self::spawn_stdio(process),
            LanguageServerTransport::Tcp => {
                // A server that lives inside another program (Godot's editor)
                // is attach-only. It owns the project's import/scan state, and
                // a second editor would compete over `.godot` instead of being
                // the editor the user is working in.
                if let Some(attach) = spec.attach.as_ref() {
                    let (host, port) = attach.resolve();
                    // Godot stores a custom language-server port in its editor
                    // settings; try it when the declared attach port is empty.
                    let discover = spec.id == "gdscript";
                    let Some(stream) = connect_attach(attach, discover) else {
                        return Err(io::Error::new(
                            io::ErrorKind::ConnectionRefused,
                            format!(
                                "no language server at {host}:{port}; open the project in the editor"
                            ),
                        ));
                    };
                    return Self::spawn_tcp_stream(None, stream);
                }
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
                processes::register(child.id());
                Self::spawn_tcp(Some(child), port)
            }
        }
    }

    fn spawn_stdio(mut process: Command) -> io::Result<Self> {
        process.stdin(Stdio::piped()).stdout(Stdio::piped());
        let mut child = process.spawn()?;
        processes::register(child.id());
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
            child: Some(child),
            outgoing,
            incoming,
            next_id: 1,
        })
    }

    /// Connects to a spawned TCP server (retrying until it listens).
    fn spawn_tcp(child: Option<Child>, port: u16) -> io::Result<Self> {
        let (outgoing, out_rx) = mpsc::channel::<Value>();
        let (events, incoming) = mpsc::channel::<ClientEvent>();
        let connect_events = events.clone();
        thread::Builder::new()
            .name("igui-lsp-connect".into())
            .spawn(
                move || match connect_loopback(port, Duration::from_secs(8)) {
                    Ok(stream) => {
                        if let Err(error) = start_tcp_io(stream, out_rx, connect_events.clone()) {
                            let _ = connect_events
                                .send(ClientEvent::ReadError(format!("LSP TCP threads: {error}")));
                        }
                    }
                    Err(error) => {
                        let _ = connect_events.send(ClientEvent::ReadError(format!(
                            "language server TCP {port}: {error}"
                        )));
                    }
                },
            )
            .map_err(|error| io::Error::other(format!("LSP connect thread: {error}")))?;

        Ok(Self {
            child,
            outgoing,
            incoming,
            next_id: 1,
        })
    }

    /// Uses an existing connection (a running editor's language server).
    fn spawn_tcp_stream(child: Option<Child>, stream: TcpStream) -> io::Result<Self> {
        let (outgoing, out_rx) = mpsc::channel::<Value>();
        let (events, incoming) = mpsc::channel::<ClientEvent>();
        start_tcp_io(stream, out_rx, events)?;
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

    /// True when we connected to someone else's server (the running editor).
    fn attached(&self) -> bool {
        self.child.is_none()
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

/// Splits one connected stream into the writer and reader threads.
fn start_tcp_io(
    stream: TcpStream,
    out_rx: Receiver<Value>,
    events: Sender<ClientEvent>,
) -> io::Result<()> {
    let read_stream = stream.try_clone()?;
    let writer_events = events.clone();
    let writer = thread::Builder::new()
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
        return Err(io::Error::other(format!("LSP writer thread: {error}")));
    }
    thread::Builder::new()
        .name("igui-lsp-reader".into())
        .spawn(move || read_events(read_stream, events))
        .map_err(|error| io::Error::other(format!("LSP reader thread: {error}")))?;
    Ok(())
}

/// Connects to a language server that is already listening (Godot's editor).
/// A short timeout keeps a missing editor from stalling the UI thread.
///
/// With `discover`, a failed default port falls back to the port stored in
/// Godot's editor settings (unless the environment override is set explicitly).
fn connect_attach(attach: &AttachTarget, discover: bool) -> Option<TcpStream> {
    let (host, port) = attach.resolve();
    if let Some(stream) = try_attach(&host, port) {
        return Some(stream);
    }
    if !discover {
        return None;
    }
    let overridden = attach
        .env
        .as_deref()
        .and_then(std::env::var_os)
        .is_some_and(|value| !value.is_empty());
    if overridden {
        return None;
    }
    let discovered = crate::services::godot::discover_lsp_port()?;
    (discovered != port)
        .then(|| try_attach(&host, discovered))
        .flatten()
}

fn try_attach(host: &str, port: u16) -> Option<TcpStream> {
    use std::net::ToSocketAddrs;

    let address = (host, port).to_socket_addrs().ok()?.next()?;
    let stream = TcpStream::connect_timeout(&address, Duration::from_millis(500)).ok()?;
    let _ = stream.set_nodelay(true);
    Some(stream)
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
        // An attached server belongs to someone else; only our own child is killed.
        if let Some(child) = self.child.as_mut() {
            processes::unregister(child.id());
            let _ = child.kill();
            let _ = child.wait();
        }
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

/// Byte range of the single-line string literal containing `caret`, including
/// its quotes. Used to replace a whole `"res://…"` path instead of inserting
/// the server's quoted insertion text at the caret.
///
/// Comments are ignored, escapes are respected, and triple-quoted strings
/// return `None` (their replacement span is not a single line).
pub fn enclosing_string_span(text: &str, caret: usize) -> Option<(usize, usize)> {
    let caret = caret.min(text.len());
    let line_start = text[..caret].rfind('\n').map_or(0, |index| index + 1);
    let line_end = text[caret..]
        .find('\n')
        .map_or(text.len(), |offset| caret + offset);
    let line = &text[line_start..line_end];
    let column = caret - line_start;
    let bytes = line.as_bytes();
    let mut index = 0usize;
    let mut open = None;
    while index < column {
        let byte = bytes[index];
        if open.is_some() {
            match byte {
                b'\\' => index += 2,
                b'"' | b'\'' => {
                    open = None;
                    index += 1;
                }
                _ => index += 1,
            }
            continue;
        }
        match byte {
            b'#' => return None,
            b'"' | b'\'' => {
                if line[index..].starts_with("\"\"\"") || line[index..].starts_with("'''") {
                    return None;
                }
                open = Some(index);
                index += 1;
            }
            _ => index += 1,
        }
    }
    let open = open?;
    let quote = bytes[open];
    let mut index = column;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            byte if byte == quote => return Some((line_start + open, line_start + index + 1)),
            _ => index += 1,
        }
    }
    Some((line_start + open, line_end))
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
pub fn workspace_binaries_enabled() -> bool {
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

/// One diagnostic with the file it belongs to (PROBLEMS panel row).
#[derive(Clone, Debug)]
pub struct ProblemEntry {
    pub path: PathBuf,
    pub diagnostic: Diagnostic,
}

/// One `workspace/didChangeWatchedFiles` registration from a server.
///
/// The pattern is a glob relative to the session root (a `RelativePattern`
/// base is folded in at registration time) and `kinds` is the LSP
/// `FileSystemWatcher.kind` bitmask: 1 create, 2 change, 4 delete.
#[derive(Clone, Debug)]
struct Watcher {
    glob: String,
    kinds: u32,
}

impl Watcher {
    fn matches(&self, relative: &str, kind: FileEventKind) -> bool {
        self.kinds & kind.watch_bit() != 0 && watch::glob_matches(&self.glob, relative)
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
    /// Occurrence ranges from `textDocument/documentHighlight`, per document.
    highlights: HashMap<PathBuf, Vec<TextRange>>,
    symbols_pending: HashSet<PathBuf>,
    symbols_in_flight: HashSet<PathBuf>,
    pending: HashMap<u64, (PendingKind, Instant)>,
    /// `(registration id, watcher)` pairs the server asked for.
    watchers: Vec<(String, Watcher)>,
    /// Native class names from `gdscript/capabilities`.
    native_classes: HashSet<String>,
    native_symbols: HashMap<(String, String), NativeSymbol>,
    /// Optional features the server advertised in `initialize`.
    capabilities: ServerCapabilities,
    last_active: Instant,
    changed: bool,
    /// Re-send every open document on the next `sync` so the server parses it
    /// against a project scan that just changed (Godot's class registry).
    resync: bool,
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
            highlights: HashMap::new(),
            symbols_pending: HashSet::new(),
            symbols_in_flight: HashSet::new(),
            pending: HashMap::new(),
            watchers: Vec::new(),
            native_classes: HashSet::new(),
            native_symbols: HashMap::new(),
            capabilities: ServerCapabilities::default(),
            last_active: Instant::now(),
            changed: false,
            resync: false,
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
        self.highlights.clear();
        self.symbols_pending.clear();
        self.symbols_in_flight.clear();
        self.pending.clear();
        // A restarted server registers its watchers again.
        self.watchers.clear();
        self.native_classes.clear();
        self.capabilities = ServerCapabilities::default();
        self.resync = false;
    }

    fn cancel_popup_requests(&mut self) {
        self.pending.retain(|_, (kind, _)| {
            !matches!(
                kind,
                PendingKind::Hover
                    | PendingKind::Completion
                    | PendingKind::CompletionResolve { .. }
                    | PendingKind::SignatureHelp
            )
        });
    }

    fn respond_to_server(&mut self, id: Value, method: &str, params: &Value) {
        let result = match method {
            // Servers that want file-watch events register here; without this
            // they never learn about files created outside the editor.
            "client/registerCapability" => {
                self.register_capabilities(params);
                Value::Null
            }
            "client/unregisterCapability" => {
                self.unregister_capabilities(params);
                Value::Null
            }
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
        if let Some(client) = self.client.as_ref() {
            client.respond(id, result);
        }
    }

    /// Records `workspace/didChangeWatchedFiles` registrations. Patterns are
    /// resolved against the session root so the poll only has to compare paths.
    fn register_capabilities(&mut self, params: &Value) {
        let Some(registrations) = params["registrations"].as_array() else {
            return;
        };
        for registration in registrations {
            if registration["method"].as_str() != Some("workspace/didChangeWatchedFiles") {
                continue;
            }
            let Some(id) = registration["id"].as_str() else {
                continue;
            };
            let Some(watchers) = registration["registerOptions"]["watchers"].as_array() else {
                continue;
            };
            for watcher in watchers {
                let (pattern, base) = watcher_glob(watcher);
                let Some(pattern) = pattern else { continue };
                let glob = resolve_watcher_glob(&self.root, base.as_deref(), &pattern);
                let kinds = watcher["kind"].as_u64().unwrap_or(7) as u32;
                self.watchers
                    .push((id.to_string(), Watcher { glob, kinds }));
            }
        }
    }

    fn unregister_capabilities(&mut self, params: &Value) {
        // The LSP field name is `unregisterations` (sic); accept both spellings.
        let ids = params
            .get("unregisterations")
            .or_else(|| params.get("unregistrations"))
            .and_then(Value::as_array);
        let Some(ids) = ids else { return };
        for registration in ids {
            // `Unregistration { id, method }`, but accept a bare string too.
            let id = registration
                .as_str()
                .or_else(|| registration["id"].as_str());
            let Some(id) = id else { continue };
            self.watchers.retain(|(registered, _)| registered != id);
        }
    }

    /// Whether this session wants `path` reported as `kind`.
    ///
    /// Project/workspace markers always belong to the session: build and config
    /// files (`compile_commands.json`, `tsconfig.json`, `Cargo.toml`, …) change
    /// analysis without being source files, and a watcher registration often
    /// covers only source extensions. Otherwise registered watchers decide;
    /// without them the spec's extensions do.
    fn wants(&self, path: &Path, kind: FileEventKind) -> bool {
        if self.spec.is_marker(path) {
            return true;
        }
        if self.watchers.is_empty() {
            return self.spec.matches_path(path);
        }
        let relative = path.strip_prefix(&self.root).unwrap_or(path);
        let relative = relative.to_string_lossy().replace('\\', "/");
        self.watchers
            .iter()
            .any(|(_, watcher)| watcher.matches(&relative, kind))
    }

    /// Whether a path is part of this session's project surface: one of its
    /// extensions or a project marker (`project.godot`).
    fn owns_structural(&self, path: &Path) -> bool {
        self.spec.matches_path(path) || self.spec.is_marker(path)
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
        self.send_position_with(method, kind, path, doc, byte, Value::Null)
    }

    /// `send_position` plus extra request fields (`references` needs a
    /// `context` object).
    fn send_position_with(
        &mut self,
        method: &str,
        kind: PendingKind,
        path: &Path,
        doc: &Document,
        byte: usize,
        extra: Value,
    ) -> bool {
        if self.status != LspStatus::Ready {
            return false;
        }
        let Some(client) = self.client.as_mut() else {
            return false;
        };
        let (line, character) = position_of_byte(doc, byte.min(doc.text().len()));
        let mut params = json!({
            "textDocument": {"uri": path_to_uri(path)},
            "position": {"line": line, "character": character},
        });
        if let (Some(params), Value::Object(extra)) = (params.as_object_mut(), extra) {
            params.extend(extra);
        }
        let id = client.request(method, params);
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
            self.highlights.remove(&path);
            self.symbols_pending.remove(&path);
            self.symbols_in_flight.remove(&path);
        }

        let now = Instant::now();
        // A structural change reached a self-rescanning server: re-send every
        // document so it parses against the refreshed project scan (Godot's
        // global class registry) instead of a stale cached parse.
        let force = std::mem::take(&mut self.resync);
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
                    if open.rev == doc.rev() && !force {
                        open.changed_at = None;
                        continue;
                    }
                    let due = force
                        || open
                            .changed_at
                            .is_some_and(|at| now.duration_since(at) >= CHANGE_DEBOUNCE);
                    if !due {
                        if open.changed_at.is_none() {
                            open.changed_at = Some(now);
                        }
                        continue;
                    }
                    open.version += 1;
                    open.rev = doc.rev();
                    open.changed_at = None;
                    // Ranges refer to the previous text; the state layer asks
                    // again once the caret settles.
                    self.highlights.remove(path);
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
    /// When open documents should be re-sent to a self-rescanning server.
    resync_at: Option<Instant>,
    /// Remaining passes for the current structural change (`RESCAN_PASSES`).
    resync_passes: u8,
    notified: HashSet<String>,
    issue: Option<LspIssue>,
    /// One-shot status line for the next frame (attached vs. spawned server).
    notice: Option<String>,
    enabled: bool,
    /// Whether repository-local `node_modules/.bin` shims may be executed.
    allow_workspace_binaries: bool,
    /// Set by [`LspController::shutdown`] so no frame can restart a server
    /// between killing children and AppKit's process exit.
    stopped: bool,
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
    pub signature_help: Option<SignatureHelpUi>,
    /// Pending result of `textDocument/references`: symbol name + locations.
    references: Option<(String, Vec<Location>)>,
    /// Context captured for prepareRename/rename, and their results.
    rename_context: Option<RenameContext>,
    prepare_result: Option<Option<String>>,
    rename_result: Option<Vec<WorkspaceTextEdit>>,
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
            resync_at: None,
            resync_passes: 0,
            notified: HashSet::new(),
            issue: None,
            notice: None,
            enabled: true,
            allow_workspace_binaries: workspace_binaries_enabled(),
            stopped: false,
            changed: false,
            hover: None,
            hover_anchor: None,
            hover_rect: igui_core::Rect::ZERO,
            hover_doc_hit: igui_core::Rect::ZERO,
            completion_doc_hit: igui_core::Rect::ZERO,
            completion: None,
            completion_prefix: String::new(),
            signature_help: None,
            references: None,
            rename_context: None,
            prepare_result: None,
            rename_result: None,
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

    /// Applies the settings/UI trust switch. Changing it restarts sessions so
    /// the new executable resolution policy takes effect.
    pub fn set_workspace_binaries(&mut self, allow: bool) {
        if self.allow_workspace_binaries == allow {
            return;
        }
        self.allow_workspace_binaries = allow;
        self.stop_all();
        self.changed = true;
    }

    /// A missing-tool prompt waiting for the UI.
    pub fn take_issue(&mut self) -> Option<LspIssue> {
        self.issue.take()
    }

    /// One-shot status line from the last session start (attach vs. spawn).
    pub fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
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

    /// Drops the server that owns `path` so the next tick starts a fresh one.
    ///
    /// For a stdio server this starts a fresh process. An attached editor
    /// (Godot) is simply reconnected: its project scan belongs to the editor,
    /// so focusing Godot is what refreshes new scripts. Diagnostics are cleared
    /// (the previous connection's results may be stale) and the server
    /// republishes them on `didOpen`. Saves and file changes never take this
    /// path; it is the explicit reset behind the **Restart Language Server**
    /// command.
    pub fn restart(&mut self, path: &Path) -> bool {
        let index = self
            .sessions
            .iter()
            .position(|session| session.spec.matches_path(path) && path_under(path, &session.root))
            .or_else(|| {
                self.sessions
                    .iter()
                    .position(|session| session.spec.matches_path(path))
            });
        let Some(index) = index else {
            return false;
        };
        let spec = self.sessions[index].spec.clone();
        let root = self.sessions[index].root.clone();
        self.sessions[index].client = None;
        self.sessions[index].status = LspStatus::Off;
        self.sessions[index].diagnostics.clear();
        self.sessions[index].reset_protocol();
        self.sessions[index].changed = true;
        self.failed.remove(&(spec.id, root));
        self.clear_transient();
        self.notice = Some(format!("{} language server restarting", spec.language));
        self.changed = true;
        true
    }

    /// Kills every language-server child and drops session state. Called on
    /// clean quit before AppKit terminates the process (AppKit's terminate path
    /// does not unwind Rust destructors).
    pub fn shutdown(&mut self) {
        self.stopped = true;
        self.stop_all();
    }

    /// Drops every project-scoped server and result while keeping language
    /// intelligence enabled for the next project.
    pub fn reset_project(&mut self) {
        self.stop_all();
        self.changed = true;
    }

    fn stop_all(&mut self) {
        self.sessions.clear();
        self.active = None;
        self.failed.clear();
        self.notified.clear();
        self.issue = None;
        self.resync_at = None;
        self.resync_passes = 0;
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
        self.signature_help = None;
        self.references = None;
        self.rename_context = None;
        self.prepare_result = None;
        self.rename_result = None;
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

    /// The optional features the active server advertised. Empty when no
    /// session is live, so callers naturally degrade to "unsupported".
    pub fn capabilities(&self) -> ServerCapabilities {
        self.active_session()
            .map(|session| session.capabilities.clone())
            .unwrap_or_default()
    }

    /// Characters that should request completion immediately (the server's
    /// own trigger list; `.` stays a trigger for every server).
    pub fn completion_triggers(&self) -> &[char] {
        static DEFAULT: [char; 1] = ['.'];
        self.active_session()
            .map(|session| session.capabilities.completion_triggers.as_slice())
            .filter(|triggers| !triggers.is_empty())
            .unwrap_or(&DEFAULT)
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
        if self.stopped {
            return;
        }
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
        // Re-parse open documents after a self-rescanning server (Godot) has
        // had time to scan a structural file change.
        self.service_resync(Instant::now());
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
        let retry_after = if spec.attach.is_some() {
            ATTACH_RETRY
        } else {
            RETRY_AFTER
        };
        if self
            .failed
            .get(&key)
            .is_some_and(|at| at.elapsed() < retry_after)
        {
            return;
        }
        match LspClient::spawn(&spec, &root, self.allow_workspace_binaries) {
            Ok(mut client) => {
                self.notice = Some(if client.attached() {
                    format!(
                        "{} language server: attached to the running editor",
                        spec.language
                    )
                } else {
                    format!(
                        "{} language server: started {}",
                        spec.language, spec.command
                    )
                });
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
                if spec.attach.is_some() && !missing {
                    // Attached servers are retried while the editor is closed;
                    // say how to fix it once instead of every retry.
                    if self.notified.insert(format!("attach:{}", spec.id)) {
                        self.notice = Some(format!("{}: {error}", spec.language));
                    }
                } else if missing && self.notified.insert(spec.id.clone()) {
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
                    let language = self.sessions[index].spec.language.clone();
                    self.sessions[index].capabilities = server_capabilities(&result, &language);
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
            PendingKind::SignatureHelp => {
                if is_active {
                    self.signature_help = parse_signature_help(&result);
                    self.changed = true;
                }
            }
            PendingKind::Highlights(path) => {
                if self.sessions[index].open_docs.contains_key(&path) {
                    let ranges = parse_text_ranges(&result);
                    if ranges.is_empty() {
                        self.sessions[index].highlights.remove(&path);
                    } else {
                        self.sessions[index].highlights.insert(path, ranges);
                    }
                    self.sessions[index].changed = true;
                }
            }
            PendingKind::References { symbol } => {
                if is_active {
                    self.references = Some((symbol, parse_locations(&result)));
                    self.changed = true;
                }
            }
            PendingKind::PrepareRename => {
                if is_active {
                    self.prepare_result = Some(parse_prepare_rename(&result));
                    self.changed = true;
                }
            }
            PendingKind::Rename => {
                if is_active {
                    self.rename_result = Some(parse_workspace_edit(&result));
                    self.changed = true;
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
        // Inside a resource path the server already filtered for the string
        // context; the identifier prefix would reject `"res://…"` inserts.
        self.completion_prefix = if enclosing_string_span(doc.text(), doc.caret()).is_some() {
            String::new()
        } else {
            completion_prefix(doc)
        };
        self.send_position("textDocument/completion", PendingKind::Completion, doc)
    }

    /// Asks for the call signature around the caret. The request is only made
    /// when the server advertised `signatureHelpProvider`.
    pub fn request_signature_help(&mut self, doc: &Document) -> bool {
        {
            let Some(session) = self.active_session_mut() else {
                return false;
            };
            if !session.capabilities.signature_help || session.status != LspStatus::Ready {
                return false;
            }
            // A newer caret position wins over an in-flight request.
            session
                .pending
                .retain(|_, (kind, _)| !matches!(kind, PendingKind::SignatureHelp));
        }
        self.send_position(
            "textDocument/signatureHelp",
            PendingKind::SignatureHelp,
            doc,
        )
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

    /// Looks up the occurrence ranges for the symbol under the caret.
    pub fn highlights_for(&self, path: &Path) -> &[TextRange] {
        self.session_for_path(path)
            .and_then(|session| session.highlights.get(path))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn clear_document_highlights(&mut self, path: &Path) {
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.highlights.contains_key(path))
        {
            session.highlights.remove(path);
            session.changed = true;
        }
    }

    /// Asks for every occurrence of the symbol at the caret. Gated on
    /// `documentHighlightProvider`.
    pub fn request_document_highlights(&mut self, doc: &Document) -> bool {
        let Some(path) = doc.path().map(Path::to_path_buf) else {
            return false;
        };
        {
            let Some(session) = self.active_session_mut() else {
                return false;
            };
            if !session.capabilities.document_highlight || session.status != LspStatus::Ready {
                return false;
            }
            session
                .pending
                .retain(|_, (kind, _)| !matches!(kind, PendingKind::Highlights(_)));
        }
        self.send_position(
            "textDocument/documentHighlight",
            PendingKind::Highlights(path),
            doc,
        )
    }

    /// Asks for every reference to the symbol at the caret.
    pub fn request_references(&mut self, doc: &Document, symbol: &str) -> bool {
        let Some(path) = doc.path().map(Path::to_path_buf) else {
            return false;
        };
        {
            let Some(session) = self.active_session_mut() else {
                return false;
            };
            if !session.capabilities.references || session.status != LspStatus::Ready {
                return false;
            }
            session
                .pending
                .retain(|_, (kind, _)| !matches!(kind, PendingKind::References { .. }));
        }
        self.references = None;
        let Some(session) = self.active_session_mut() else {
            return false;
        };
        session.send_position_with(
            "textDocument/references",
            PendingKind::References {
                symbol: symbol.to_string(),
            },
            &path,
            doc,
            doc.caret(),
            json!({"context": {"includeDeclaration": true}}),
        )
    }

    pub fn take_references(&mut self) -> Option<(String, Vec<Location>)> {
        self.references.take()
    }

    /// Drops an open references list and its in-flight request.
    pub fn cancel_references(&mut self) {
        self.references = None;
        if let Some(session) = self.active_session_mut() {
            session
                .pending
                .retain(|_, (kind, _)| !matches!(kind, PendingKind::References { .. }));
        }
        self.changed = true;
    }

    /// Drops a visible signature card and its in-flight request (Escape or `)`).
    pub fn cancel_signature_help(&mut self) {
        self.signature_help = None;
        if let Some(session) = self.active_session_mut() {
            session
                .pending
                .retain(|_, (kind, _)| !matches!(kind, PendingKind::SignatureHelp));
        }
        self.changed = true;
    }

    /// Drops a rename prompt's in-flight prepare/rename request and results.
    pub fn cancel_rename(&mut self) {
        self.rename_context = None;
        self.prepare_result = None;
        self.rename_result = None;
        if let Some(session) = self.active_session_mut() {
            session.pending.retain(|_, (kind, _)| {
                !matches!(kind, PendingKind::PrepareRename | PendingKind::Rename)
            });
        }
        self.changed = true;
    }

    /// `F2` step one: ask whether the symbol can be renamed. Returns `None`
    /// until the server answers.
    pub fn request_prepare_rename(&mut self, doc: &Document, old_name: &str) -> bool {
        let Some(path) = doc.path().map(Path::to_path_buf) else {
            return false;
        };
        {
            let Some(session) = self.active_session_mut() else {
                return false;
            };
            if !session.capabilities.prepare_rename || session.status != LspStatus::Ready {
                return false;
            }
            session
                .pending
                .retain(|_, (kind, _)| !matches!(kind, PendingKind::PrepareRename));
        }
        self.prepare_result = None;
        self.rename_context = Some(RenameContext {
            doc_id: doc.id(),
            rev: doc.rev(),
            path,
            caret: doc.caret(),
            old_name: old_name.to_string(),
            new_name: String::new(),
        });
        self.send_position(
            "textDocument/prepareRename",
            PendingKind::PrepareRename,
            doc,
        )
    }

    /// The prepare result plus the context, or `None` while still pending.
    /// `Some(None)` means the server refused (native symbol, no rename).
    pub fn take_prepare_rename(&mut self) -> Option<(RenameContext, Option<String>)> {
        let result = self.prepare_result.take()?;
        let context = self.rename_context.take()?;
        Some((context, result))
    }

    /// `F2` step two: ask for the workspace edit using a new name.
    pub fn request_rename(&mut self, doc: &Document, new_name: &str, old_name: &str) -> bool {
        let Some(path) = doc.path().map(Path::to_path_buf) else {
            return false;
        };
        {
            let Some(session) = self.active_session_mut() else {
                return false;
            };
            if !session.capabilities.rename || session.status != LspStatus::Ready {
                return false;
            }
            session
                .pending
                .retain(|_, (kind, _)| !matches!(kind, PendingKind::Rename));
        }
        self.rename_result = None;
        self.rename_context = Some(RenameContext {
            doc_id: doc.id(),
            rev: doc.rev(),
            path: path.clone(),
            caret: doc.caret(),
            old_name: old_name.to_string(),
            new_name: new_name.to_string(),
        });
        let Some(session) = self.active_session_mut() else {
            return false;
        };
        session.send_position_with(
            "textDocument/rename",
            PendingKind::Rename,
            &path,
            doc,
            doc.caret(),
            json!({"newName": new_name}),
        )
    }

    /// The edit list plus the context captured at request time.
    pub fn take_rename(&mut self) -> Option<(RenameContext, Vec<WorkspaceTextEdit>)> {
        let edits = self.rename_result.take()?;
        let context = self.rename_context.take()?;
        Some((context, edits))
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

    /// Every diagnostic from every live session, sorted by file, position and
    /// severity. The PROBLEMS panel snapshots this on change instead of
    /// recomputing it every frame.
    pub fn workspace_diagnostics(&self) -> Vec<ProblemEntry> {
        let mut out = Vec::new();
        for session in &self.sessions {
            for (path, diagnostics) in &session.diagnostics {
                out.extend(diagnostics.iter().map(|diagnostic| ProblemEntry {
                    path: path.clone(),
                    diagnostic: diagnostic.clone(),
                }));
            }
        }
        out.sort_by(|a, b| {
            a.path
                .cmp(&b.path)
                .then(a.diagnostic.start_line.cmp(&b.diagnostic.start_line))
                .then(a.diagnostic.start_char.cmp(&b.diagnostic.start_char))
                .then(a.diagnostic.severity.cmp(&b.diagnostic.severity))
        });
        out
    }

    /// Project roots of servers that refresh themselves from their own scan.
    /// The filesystem poll watches each root's `.godot` directory so a class
    /// registry rewrite (even from an unseen subtree) still re-parses documents.
    pub fn rescan_roots(&self) -> Vec<PathBuf> {
        self.sessions
            .iter()
            .filter(|session| session.spec.file_changes == FileChangePolicy::ServerRescan)
            .map(|session| session.root.clone())
            .collect()
    }

    /// Forwards file changes (editor-side and polled) to every ready server.
    ///
    /// Events are deduped by path (later kinds win) and filtered per session:
    /// registered watchers decide, otherwise the spec's extensions do. A
    /// [`FileChangePolicy::ServerRescan`] server ignores notifications, so the
    /// controller instead schedules a re-parse of the open documents once its
    /// own project scan has run. Diagnostics for deleted files are dropped
    /// immediately: no server will ever publish an update for them again.
    pub fn notify_file_events(&mut self, events: &[FileEvent]) {
        if events.is_empty() {
            return;
        }
        for event in events {
            if event.kind == FileEventKind::Deleted && !event.path.exists() {
                self.forget_path(&event.path);
            }
        }
        let now = Instant::now();
        for session in &mut self.sessions {
            let under_root = events
                .iter()
                .any(|event| path_under(&event.path, &session.root));
            if !under_root {
                continue;
            }
            if session.spec.file_changes == FileChangePolicy::ServerRescan {
                // Godot's LSP has no watched-file handler. Its scan timer (and
                // the class cache it rewrites) is the real signal; the open
                // documents must then be parsed again. A restart would drop
                // the process (a full startup scan), so the documents are
                // re-sent instead.
                let relevant = |event: &FileEvent| {
                    path_under(&event.path, &session.root) && rescans_project(session, event)
                };
                if events.iter().any(relevant) {
                    let at = now + RESCAN_DELAY;
                    self.resync_at = Some(self.resync_at.map_or(at, |current| current.min(at)));
                    self.resync_passes = self.resync_passes.max(RESCAN_PASSES);
                }
                continue;
            }
            let Some(client) = session.client.as_ref() else {
                continue;
            };
            let mut changes: BTreeMap<&Path, FileEventKind> = BTreeMap::new();
            for event in events {
                if !path_under(&event.path, &session.root) {
                    continue;
                }
                if !session.wants(&event.path, event.kind) {
                    continue;
                }
                changes.insert(event.path.as_path(), event.kind);
            }
            if changes.is_empty() {
                continue;
            }
            let changes: Vec<Value> = changes
                .into_iter()
                .map(|(path, kind)| json!({"uri": path_to_uri(path), "type": kind.lsp_type()}))
                .collect();
            client.notify(
                "workspace/didChangeWatchedFiles",
                json!({ "changes": changes }),
            );
        }
    }

    /// Drops every cached result for `path` (deleted file or directory).
    ///
    /// `starts_with` is component-wise, so a deleted folder also clears the
    /// diagnostics of open files inside it without matching `a.gd.bak`.
    fn forget_path(&mut self, path: &Path) {
        for session in &mut self.sessions {
            let before = session.diagnostics.len();
            session
                .diagnostics
                .retain(|cached, _| !cached.starts_with(path));
            session
                .semantic
                .retain(|cached, _| !cached.starts_with(path));
            session
                .semantic_pending
                .retain(|cached| !cached.starts_with(path));
            session
                .semantic_in_flight
                .retain(|cached| !cached.starts_with(path));
            session
                .symbols
                .retain(|cached, _| !cached.starts_with(path));
            session
                .symbols_pending
                .retain(|cached| !cached.starts_with(path));
            session
                .symbols_in_flight
                .retain(|cached| !cached.starts_with(path));
            if session.diagnostics.len() != before {
                session.changed = true;
            }
        }
    }

    /// Re-parses every open document of a self-rescanning server once its
    /// project scan has picked up a structural change.
    ///
    /// Called from [`Self::tick`]; `next_wake` keeps the app awake until then.
    /// The server keeps running: a fresh process would pay for a full startup
    /// scan and leave the editor without diagnostics while it does.
    fn service_resync(&mut self, now: Instant) {
        let Some(at) = self.resync_at else {
            return;
        };
        if now < at {
            return;
        }
        self.resync_at = None;
        for session in &mut self.sessions {
            if session.spec.file_changes == FileChangePolicy::ServerRescan
                && session.status == LspStatus::Ready
            {
                session.resync = true;
            }
        }
        // A big project may still have been scanning when the first pass ran.
        if self.resync_passes > 1 {
            self.resync_passes -= 1;
            self.resync_at = Some(now + RESCAN_FOLLOWUP);
        } else {
            self.resync_passes = 0;
        }
    }

    /// Earliest instant at which open documents must be re-sent.
    pub fn next_wake(&self) -> Option<Instant> {
        self.resync_at
    }

    /// `textDocument/didSave` for the sessions that have `path` open.
    ///
    /// A save is incremental: the document content is already synchronized, so
    /// the server only needs the save marker. Saves never restart a server
    /// (autosave would keep a headless Godot down); a changed `class_name`
    /// registry is picked up through the project scan and class-cache watch.
    pub fn notify_doc_saved(&mut self, path: &Path) {
        for session in &self.sessions {
            if !session.open_docs.contains_key(path) {
                continue;
            }
            if let Some(client) = session.client.as_ref() {
                client.notify(
                    "textDocument/didSave",
                    json!({"textDocument": {"uri": path_to_uri(path)}}),
                );
            }
        }
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

/// Reads the optional features an `initialize` result advertises.
///
/// Godot publishes `%` as a working node-reference trigger but not in its
/// declared list, so GDScript gets that one extra character.
fn server_capabilities(result: &Value, language: &str) -> ServerCapabilities {
    let capabilities = &result["capabilities"];
    let provider_enabled = |key: &str| {
        capabilities
            .get(key)
            .is_some_and(|value| !value.is_null() && value.as_bool() != Some(false))
    };
    let mut completion_triggers = Vec::new();
    if let Some(items) = capabilities["completionProvider"]["triggerCharacters"].as_array() {
        for item in items {
            let Some(value) = item.as_str() else { continue };
            let mut chars = value.chars();
            if let (Some(ch), None) = (chars.next(), chars.next()) {
                if !ch.is_control() && !completion_triggers.contains(&ch) {
                    completion_triggers.push(ch);
                }
            }
        }
    }
    if language == "gdscript" && !completion_triggers.contains(&'%') {
        completion_triggers.push('%');
    }
    let rename_provider = &capabilities["renameProvider"];
    let rename = !rename_provider.is_null() && rename_provider.as_bool() != Some(false);
    let prepare_rename = rename_provider["prepareProvider"].as_bool() == Some(true);
    ServerCapabilities {
        completion_triggers,
        signature_help: provider_enabled("signatureHelpProvider"),
        references: provider_enabled("referencesProvider"),
        document_highlight: provider_enabled("documentHighlightProvider"),
        rename,
        prepare_rename,
    }
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

/// Splits one LSP `FileSystemWatcher` into its glob and (optional) base path.
fn watcher_glob(watcher: &Value) -> (Option<String>, Option<PathBuf>) {
    match &watcher["globPattern"] {
        Value::String(pattern) => (Some(pattern.clone()), None),
        // `RelativePattern { baseUri, pattern }` from a server that ignored our
        // `relativePatternSupport: false`.
        Value::Object(_) => {
            let pattern = watcher["globPattern"]["pattern"]
                .as_str()
                .map(str::to_string);
            let base = watcher["globPattern"]["baseUri"]
                .as_str()
                .and_then(uri_to_path);
            (pattern, base)
        }
        _ => (None, None),
    }
}

/// Folds a watcher's base into a pattern relative to the session root.
fn resolve_watcher_glob(root: &Path, base: Option<&Path>, pattern: &str) -> String {
    let pattern = pattern.replace('\\', "/");
    let base = base.filter(|base| *base != root);
    let Some(base) = base else {
        return pattern;
    };
    let prefix = base
        .strip_prefix(root)
        .unwrap_or(base)
        .to_string_lossy()
        .replace('\\', "/");
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() {
        pattern
    } else {
        format!("{prefix}/{pattern}")
    }
}

/// Whether a file event can change the server's project-wide view.
///
/// Content-only edits of open files are already described by their documents;
/// reacting to them would make autosave re-parse every open tab. Godot's class
/// cache rewrite counts because it means the global `class_name` registry
/// changed even when the edited file is outside the polled directories.
fn rescans_project(session: &Session, event: &FileEvent) -> bool {
    // The class cache is the engine's own "the registry changed" signal, so any
    // kind counts: a slow first scan may create it after the file event.
    event.path.file_name().and_then(|name| name.to_str()) == Some(CLASS_CACHE_FILE)
        || structural_change(session, event)
}

/// Created/deleted script or project marker: the class registry may change.
///
/// Content-only edits are covered by the open document and the class cache is
/// deliberately excluded: the scan rewrites it, so reacting to it would loop.
fn structural_change(session: &Session, event: &FileEvent) -> bool {
    matches!(event.kind, FileEventKind::Created | FileEventKind::Deleted)
        && session.owns_structural(&event.path)
}

/// True when `path` is `root` or below it. The polled path may be the
/// non-canonical spelling of a symlinked root (`/var` vs `/private/var`).
fn path_under(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
        || path
            .canonicalize()
            .map(|canonical| canonical.starts_with(root))
            .unwrap_or(false)
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
            "workspace": {
                "workspaceFolders": true,
                "configuration": true,
                // File watches: servers register patterns, the poll reports the
                // files, so an externally created script reaches the server.
                "didChangeWatchedFiles": {
                    "dynamicRegistration": true,
                    "relativePatternSupport": false
                }
            },
            "textDocument": {
                "synchronization": {
                    "dynamicRegistration": false,
                    "didSave": true,
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

fn parse_signature_help(result: &Value) -> Option<SignatureHelpUi> {
    let items = result["signatures"].as_array()?;
    let mut signatures = Vec::new();
    for item in items.iter().take(8) {
        let Some(label) = item["label"].as_str().filter(|label| !label.is_empty()) else {
            continue;
        };
        let mut parameters = Vec::new();
        if let Some(items) = item["parameters"].as_array() {
            for parameter in items.iter().take(64) {
                let span = match &parameter["label"] {
                    Value::String(text) => {
                        label.find(text).map(|start| (start, start + text.len()))
                    }
                    Value::Array(range) if range.len() == 2 => {
                        let mut start = range[0].as_u64().unwrap_or(0) as usize;
                        let mut end = range[1].as_u64().unwrap_or(0) as usize;
                        start = start.min(label.len());
                        end = end.min(label.len());
                        while start > 0 && !label.is_char_boundary(start) {
                            start -= 1;
                        }
                        while end > 0 && !label.is_char_boundary(end) {
                            end -= 1;
                        }
                        (start < end).then_some((start, end))
                    }
                    _ => None,
                };
                if let Some(span) = span {
                    parameters.push(span);
                }
            }
        }
        signatures.push(Signature {
            label: label.to_string(),
            documentation: completion_documentation(item),
            parameters,
        });
    }
    if signatures.is_empty() {
        return None;
    }
    let active_signature =
        (result["activeSignature"].as_u64().unwrap_or(0) as usize).min(signatures.len() - 1);
    let active_parameter = if signatures[active_signature].parameters.is_empty() {
        0
    } else {
        (result["activeParameter"].as_u64().unwrap_or(0) as usize)
            .min(signatures[active_signature].parameters.len() - 1)
    };
    Some(SignatureHelpUi {
        signatures,
        active_signature,
        active_parameter,
    })
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

/// `prepareRename` answers with `{range, placeholder?}` or a bare range.
/// `None` means the server cannot rename this symbol.
fn parse_prepare_rename(result: &Value) -> Option<String> {
    if result.is_null() {
        return None;
    }
    if result.get("start").is_some() {
        return Some(String::new());
    }
    if result.get("range").is_some() {
        return Some(
            result["placeholder"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    None
}

/// Folds `workspace/rename`'s `changes` map and `documentChanges` array into
/// one flat, bounded edit list.
fn parse_workspace_edit(result: &Value) -> Vec<WorkspaceTextEdit> {
    let mut edits = Vec::new();
    let mut push_edits = |uri: Option<&str>, items: Option<&Vec<Value>>| {
        let Some(uri) = uri else { return };
        let Some(path) = uri_to_path(uri) else { return };
        let Some(items) = items else { return };
        for item in items.iter().take(2_000) {
            let range = &item["range"];
            let start = &range["start"];
            let end = &range["end"];
            let (Some(sl), Some(sc), Some(el), Some(ec)) = (
                start["line"].as_u64(),
                start["character"].as_u64(),
                end["line"].as_u64(),
                end["character"].as_u64(),
            ) else {
                continue;
            };
            let Some(new_text) = item["newText"].as_str() else {
                continue;
            };
            edits.push(WorkspaceTextEdit {
                path: path.clone(),
                start_line: sl as u32,
                start_char: sc as u32,
                end_line: el as u32,
                end_char: ec as u32,
                new_text: new_text.to_string(),
            });
            if edits.len() >= 4_000 {
                return;
            }
        }
    };
    if let Some(changes) = result["changes"].as_object() {
        for (uri, items) in changes {
            push_edits(Some(uri), items.as_array());
        }
    }
    if let Some(changes) = result["documentChanges"].as_array() {
        for change in changes.iter().take(100) {
            let uri = change["textDocument"]["uri"].as_str();
            push_edits(uri, change["edits"].as_array());
        }
    }
    edits
}

fn parse_text_ranges(result: &Value) -> Vec<TextRange> {
    let Some(items) = result.as_array() else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    for item in items.iter().take(1_000) {
        let range = &item["range"];
        let start = &range["start"];
        let end = &range["end"];
        let (Some(sl), Some(sc), Some(el), Some(ec)) = (
            start["line"].as_u64(),
            start["character"].as_u64(),
            end["line"].as_u64(),
            end["character"].as_u64(),
        ) else {
            continue;
        };
        if (sl, sc) >= (el, ec) {
            continue;
        }
        ranges.push(TextRange {
            start_line: sl as u32,
            start_char: sc as u32,
            end_line: el as u32,
            end_char: ec as u32,
        });
    }
    ranges
}

fn parse_locations(result: &Value) -> Vec<Location> {
    let Some(items) = result.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.iter().take(500) {
        let Some(uri) = item["uri"].as_str().or_else(|| item["targetUri"].as_str()) else {
            continue;
        };
        let Some(path) = uri_to_path(uri) else {
            continue;
        };
        let range = if item.get("range").is_some() {
            &item["range"]
        } else {
            &item["targetSelectionRange"]
        };
        let start = &range["start"];
        let (Some(line), Some(character)) = (start["line"].as_u64(), start["character"].as_u64())
        else {
            continue;
        };
        out.push(Location {
            path,
            line: line as u32,
            character: character as u32,
        });
    }
    out
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
    fn initialize_advertises_file_watches_and_saves() {
        let spec = crate::services::go::go_server();
        let params = initialize_params(Path::new("/tmp/project"), &spec);
        let workspace = &params["capabilities"]["workspace"];
        assert_eq!(workspace["workspaceFolders"], json!(true));
        assert_eq!(
            workspace["didChangeWatchedFiles"]["dynamicRegistration"],
            json!(true)
        );
        assert_eq!(
            params["capabilities"]["textDocument"]["synchronization"]["didSave"],
            json!(true)
        );
    }

    #[test]
    fn watcher_globs_are_resolved_against_the_session_root() {
        let root = Path::new("/root/project");
        assert_eq!(
            resolve_watcher_glob(root, None, "**/*.gd"),
            "**/*.gd".to_string()
        );
        assert_eq!(
            resolve_watcher_glob(root, Some(root), "**/*.gd"),
            "**/*.gd".to_string()
        );
        assert_eq!(
            resolve_watcher_glob(root, Some(&root.join("scripts")), "**/*.gd"),
            "scripts/**/*.gd".to_string()
        );
        // A base outside the root keeps its absolute spelling; the matcher
        // compares absolute patterns against absolute paths.
        assert_eq!(
            resolve_watcher_glob(root, Some(Path::new("/elsewhere")), "**/*.ts"),
            "/elsewhere/**/*.ts".to_string()
        );
    }

    #[test]
    fn registered_watchers_filter_file_events() {
        let spec = crate::services::go::go_server();
        let mut session = Session::new(spec, PathBuf::from("/root"));

        // Without a registration the spec's extension decides.
        assert!(session.wants(Path::new("/root/a.go"), FileEventKind::Created));
        assert!(!session.wants(Path::new("/root/a.gd"), FileEventKind::Created));

        session.register_capabilities(&json!({"registrations": [{
            "id": "watch-gd",
            "method": "workspace/didChangeWatchedFiles",
            "registerOptions": {"watchers": [
                {"globPattern": "**/*.gd"},
                {"globPattern": {"baseUri": "file:///root/sub", "pattern": "**/*.ts"}, "kind": 4}
            ]}
        }]}));
        assert_eq!(session.watchers.len(), 2);
        // A watcher replaces the extension fallback: `.go` files are no longer
        // reported unless a pattern says so.
        assert!(!session.wants(Path::new("/root/a.go"), FileEventKind::Created));
        // Config markers are not source files but always reach the server.
        assert!(session.wants(Path::new("/root/go.mod"), FileEventKind::Changed));
        assert!(session.wants(Path::new("/root/sub/go.work"), FileEventKind::Created));
        assert!(session.wants(Path::new("/root/scripts/a.gd"), FileEventKind::Deleted));
        // The `kind: 4` watcher only reports deletions.
        assert!(session.wants(Path::new("/root/sub/x.ts"), FileEventKind::Deleted));
        assert!(!session.wants(Path::new("/root/sub/x.ts"), FileEventKind::Changed));
        assert!(!session.wants(Path::new("/root/x.ts"), FileEventKind::Deleted));

        session.unregister_capabilities(&json!({"unregisterations": [
            {"id": "watch-gd", "method": "workspace/didChangeWatchedFiles"}
        ]}));
        assert!(session.watchers.is_empty());
        assert!(session.wants(Path::new("/root/a.go"), FileEventKind::Created));
    }

    #[test]
    fn marker_files_always_reach_the_server() {
        let session = Session::new(
            crate::services::clang::clang_server(),
            PathBuf::from("/root"),
        );
        // clangd's build database and config are not source files, but they
        // change analysis and must not be filtered out by extension.
        for name in ["compile_commands.json", "compile_flags.txt", ".clangd"] {
            assert!(session.wants(&Path::new("/root").join(name), FileEventKind::Changed));
        }
        assert!(!session.wants(Path::new("/root/README.md"), FileEventKind::Changed));
    }

    #[test]
    fn path_under_accepts_the_canonical_spelling() {
        let root = Path::new("/tmp/does-not-exist/root");
        assert!(path_under(&root.join("a.gd"), root));
        assert!(!path_under(Path::new("/tmp/other/a.gd"), root));
    }

    fn diagnostic(message: &str) -> Diagnostic {
        Diagnostic {
            start_line: 0,
            start_char: 0,
            end_line: 0,
            end_char: 1,
            severity: 1,
            message: message.into(),
        }
    }

    #[test]
    fn a_deleted_file_loses_its_cached_results_at_once() {
        let spec = crate::services::godot::godot_server();
        let root = std::env::temp_dir().join(format!("igui-lsp-deleted-{}", std::process::id()));
        let mut lsp = LspController::default();
        lsp.sessions.push(Session::new(spec, root.clone()));
        let path = root.join("gone.gd");
        lsp.sessions[0]
            .diagnostics
            .insert(path.clone(), vec![diagnostic("gone")]);
        lsp.sessions[0].semantic.insert(path.clone(), Vec::new());
        lsp.sessions[0].symbols.insert(path.clone(), Vec::new());

        // A deleted folder clears everything inside it, but a name that only
        // shares the prefix must survive.
        let nested = root.join("scripts").join("enemy.gd");
        let sibling = root.join("scripts.gd");
        lsp.sessions[0]
            .diagnostics
            .insert(nested.clone(), vec![diagnostic("nested")]);
        lsp.sessions[0]
            .diagnostics
            .insert(sibling.clone(), vec![diagnostic("sibling")]);

        lsp.notify_file_events(&[
            FileEvent {
                path: path.clone(),
                kind: FileEventKind::Deleted,
            },
            FileEvent {
                path: root.join("scripts"),
                kind: FileEventKind::Deleted,
            },
        ]);

        assert!(lsp.diagnostics_for(&path).is_empty());
        assert!(!lsp.sessions[0].semantic.contains_key(&path));
        assert!(!lsp.sessions[0].symbols.contains_key(&path));
        assert!(lsp.diagnostics_for(&nested).is_empty());
        assert_eq!(lsp.diagnostics_for(&sibling).len(), 1);
        assert!(lsp.take_changed(), "PROBLEMS must be rebuilt");
    }

    #[test]
    fn a_self_rescanning_server_reparses_documents_instead_of_watched_notifications() {
        let spec = crate::services::godot::godot_server();
        let root = PathBuf::from("/tmp/igui-rescan-root");
        let mut lsp = LspController::default();
        lsp.sessions.push(Session::new(spec, root.clone()));
        lsp.sessions[0].status = LspStatus::Ready;

        // Content-only changes and Godot's own `.gd.uid` companion do not
        // invalidate the class registry: react to them only through the open
        // document itself, never by re-parsing every tab.
        for (name, kind) in [
            ("player.gd", FileEventKind::Changed),
            ("player.gd.uid", FileEventKind::Created),
        ] {
            lsp.notify_file_events(&[FileEvent {
                path: root.join(name),
                kind,
            }]);
            assert!(lsp.next_wake().is_none(), "{name} must not force a rescan");
        }

        lsp.notify_file_events(&[FileEvent {
            path: root.join("player.gd"),
            kind: FileEventKind::Created,
        }]);
        let deadline = lsp.next_wake().expect("a rescan deadline");
        assert!(!lsp.sessions[0].resync);
        lsp.service_resync(deadline + Duration::from_millis(1));
        assert!(lsp.sessions[0].resync, "open documents must be re-sent");
        // Follow-up passes cover projects whose first scan was still running.
        let followup = lsp.next_wake().expect("a follow-up pass");
        lsp.service_resync(followup + Duration::from_millis(1));
        let last = lsp.next_wake().expect("the last follow-up pass");
        lsp.service_resync(last + Duration::from_millis(1));
        assert!(lsp.next_wake().is_none());
    }

    #[test]
    fn only_structural_script_changes_trigger_a_project_rescan() {
        let spec = crate::services::godot::godot_server();
        let session = Session::new(spec, PathBuf::from("/root"));
        let event = |name: &str, kind| FileEvent {
            path: PathBuf::from("/root").join(name),
            kind,
        };
        assert!(rescans_project(
            &session,
            &event("player.gd", FileEventKind::Created)
        ));
        assert!(rescans_project(
            &session,
            &event("player.gd", FileEventKind::Deleted)
        ));
        assert!(rescans_project(
            &session,
            &event("project.godot", FileEventKind::Created)
        ));
        for kind in [
            FileEventKind::Created,
            FileEventKind::Changed,
            FileEventKind::Deleted,
        ] {
            assert!(rescans_project(
                &session,
                &event(".godot/global_script_class_cache.cfg", kind)
            ));
            // The class cache is a scan artefact, not a structural change.
            assert!(!structural_change(
                &session,
                &event(".godot/global_script_class_cache.cfg", kind)
            ));
        }
        assert!(!rescans_project(
            &session,
            &event("player.gd", FileEventKind::Changed)
        ));
        assert!(!rescans_project(
            &session,
            &event("player.gd.uid", FileEventKind::Created)
        ));
        assert!(!rescans_project(
            &session,
            &event("notes.md", FileEventKind::Created)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_resync_resends_open_documents_as_full_text_changes() {
        use std::io::BufReader;

        let root = std::env::temp_dir().join(format!("igui-lsp-resync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("player.gd");
        std::fs::write(&script, "extends Node\n\nvar speed := 1\n").unwrap();
        let script = std::fs::canonicalize(&script).unwrap();

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let spec = LanguageServerSpec {
            id: "fake-rescan".into(),
            language: "gdscript".into(),
            extensions: vec!["gd".into()],
            project_markers: vec!["project.godot".into()],
            workspace_markers: vec![],
            language_ids: Default::default(),
            loose_files: false,
            command: "/bin/sh".into(),
            command_env: None,
            args: vec!["-c".into(), "sleep 30".into()],
            transport: LanguageServerTransport::Tcp,
            port_arg: None,
            port: Some(port),
            attach: Some(AttachTarget {
                host: "127.0.0.1".into(),
                port,
                env: None,
            }),
            file_changes: FileChangePolicy::ServerRescan,
            install: None,
        };
        let mut ws = Workspace::new(&root);
        ws.open(&script).unwrap();
        let mut session = Session::new(spec, root.clone());
        session.client = Some(LspClient::spawn(&session.spec, &root, false).unwrap());
        session.status = LspStatus::Ready;
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(stream);

        session.sync(std::slice::from_ref(&script), &ws);
        let opened = read_message(&mut reader).unwrap().expect("didOpen");
        assert_eq!(opened["method"], "textDocument/didOpen");

        session.resync = true;
        session.sync(std::slice::from_ref(&script), &ws);
        let mut resent = None;
        while let Ok(Some(message)) = read_message(&mut reader) {
            if message["method"] == "textDocument/didChange" {
                resent = Some(message);
                break;
            }
        }
        let resent = resent.expect("the document was not re-sent");
        assert_eq!(resent["params"]["textDocument"]["version"], 2);
        assert_eq!(
            resent["params"]["contentChanges"][0]["text"],
            "extends Node\n\nvar speed := 1\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn structural_changes_resync_the_godot_server_without_restarting_it() {
        let spec = crate::services::godot::godot_server();
        let root = std::env::temp_dir().join(format!("igui-lsp-resync-{}", std::process::id()));
        let path = root.join("player.gd");
        let mut lsp = LspController::default();
        lsp.sessions.push(Session::new(spec, root.clone()));
        lsp.sessions[0].status = LspStatus::Ready;
        lsp.sessions[0].open_docs.insert(
            path.clone(),
            OpenDoc {
                version: 1,
                rev: 0,
                changed_at: None,
            },
        );
        lsp.sessions[0]
            .diagnostics
            .insert(path.clone(), vec![diagnostic("old")]);

        // A created script schedules a re-parse; the process keeps running so
        // diagnostics and warm project state survive.
        lsp.notify_file_events(&[FileEvent {
            path: root.join("new.gd"),
            kind: FileEventKind::Created,
        }]);
        let at = lsp.next_wake().expect("a rescan deadline");
        assert_eq!(lsp.sessions[0].status, LspStatus::Ready);
        assert!(!lsp.sessions[0].resync);
        lsp.service_resync(at + Duration::from_millis(1));
        assert!(lsp.sessions[0].resync, "open documents must be re-sent");
        assert_eq!(lsp.sessions[0].status, LspStatus::Ready);
        assert!(!lsp.diagnostics_for(&path).is_empty());

        // Follow-up passes cover a project whose first scan was still running.
        let mut passes = 0;
        while let Some(pass) = lsp.next_wake() {
            lsp.service_resync(pass + Duration::from_millis(1));
            passes += 1;
            assert!(passes <= 2, "resync passes must be bounded");
        }
        assert_eq!(passes, 2);
        assert_eq!(lsp.sessions[0].status, LspStatus::Ready);
    }

    #[test]
    fn saves_never_restart_a_rescanning_server() {
        let spec = crate::services::godot::godot_server();
        let root = PathBuf::from("/tmp/igui-lsp-glentle-root");
        let path = root.join("player.gd");
        let mut lsp = LspController::default();
        lsp.sessions.push(Session::new(spec, root.clone()));
        lsp.sessions[0].status = LspStatus::Ready;
        lsp.sessions[0].open_docs.insert(
            path.clone(),
            OpenDoc {
                version: 1,
                rev: 0,
                changed_at: None,
            },
        );
        lsp.notify_doc_saved(&path);
        assert!(lsp.next_wake().is_none());
        assert_eq!(lsp.sessions[0].status, LspStatus::Ready);
    }

    #[test]
    fn restart_drops_the_attached_session() {
        let spec = crate::services::godot::godot_server();
        let root = std::env::temp_dir().join(format!("igui-lsp-restart-{}", std::process::id()));
        let path = root.join("player.gd");
        let mut lsp = LspController::default();
        lsp.sessions.push(Session::new(spec, root.clone()));
        lsp.sessions[0].status = LspStatus::Ready;
        lsp.sessions[0]
            .diagnostics
            .insert(path.clone(), vec![diagnostic("old")]);

        assert!(lsp.restart(&path));
        assert_eq!(lsp.sessions[0].status, LspStatus::Off);
        assert!(lsp.sessions[0].diagnostics.is_empty());
        assert!(lsp.next_wake().is_none());
        assert!(
            lsp.take_notice().is_some(),
            "the UI is told about the restart"
        );
        assert!(!lsp.restart(Path::new("/elsewhere/other.rs")));
    }

    #[cfg(unix)]
    #[test]
    fn attached_servers_have_no_headless_fallback() {
        // A reserved-then-closed port: the editor is not running.
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let spec = LanguageServerSpec {
            id: "fake-attach-only".into(),
            language: "gdscript".into(),
            extensions: vec!["gd".into()],
            project_markers: vec![],
            workspace_markers: vec![],
            language_ids: Default::default(),
            loose_files: false,
            command: "/bin/sh".into(),
            command_env: None,
            args: vec!["-c".into(), "sleep 30".into()],
            transport: LanguageServerTransport::Tcp,
            port_arg: None,
            port: None,
            attach: Some(AttachTarget {
                host: "127.0.0.1".into(),
                port,
                env: None,
            }),
            file_changes: FileChangePolicy::ServerRescan,
            install: None,
        };
        let error = LspClient::spawn(&spec, Path::new("/tmp"), false)
            .err()
            .expect("nothing listens, so attach fails");
        assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused);
        assert!(error.to_string().contains(&port.to_string()));
        assert!(error.to_string().contains("open the project in the editor"));
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
            attach: None,
            file_changes: FileChangePolicy::Notify,
            install: None,
        };
        let mut client = LspClient::spawn(&spec, Path::new("/tmp"), false).unwrap();
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
    fn attach_env_overrides_the_address() {
        let name = "IGUI_EDITOR_TEST_ATTACH_ADDRESS";
        let attach = AttachTarget {
            host: "127.0.0.1".into(),
            port: 6005,
            env: Some(name.into()),
        };
        std::env::remove_var(name);
        assert_eq!(attach.resolve(), ("127.0.0.1".to_string(), 6005));
        std::env::set_var(name, "127.0.0.2:7000");
        assert_eq!(attach.resolve(), ("127.0.0.2".to_string(), 7000));
        std::env::set_var(name, "7001");
        assert_eq!(attach.resolve(), ("127.0.0.1".to_string(), 7001));
        std::env::set_var(name, ":7002");
        assert_eq!(attach.resolve(), ("127.0.0.1".to_string(), 7002));
        std::env::set_var(name, "not-a-port");
        assert_eq!(attach.resolve(), ("127.0.0.1".to_string(), 6005));
        std::env::remove_var(name);
    }

    #[cfg(unix)]
    #[test]
    fn attach_uses_a_running_server_without_spawning_a_child() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let spec = LanguageServerSpec {
            id: "fake-attach".into(),
            language: "gdscript".into(),
            extensions: vec!["gd".into()],
            project_markers: vec![],
            workspace_markers: vec![],
            language_ids: Default::default(),
            loose_files: false,
            // Must never be spawned: an attached server belongs to the editor.
            command: "/bin/sh".into(),
            command_env: None,
            args: vec!["-c".into(), "sleep 30".into()],
            transport: LanguageServerTransport::Tcp,
            port_arg: None,
            port: Some(port),
            attach: Some(AttachTarget {
                host: "127.0.0.1".into(),
                port,
                env: None,
            }),
            file_changes: FileChangePolicy::Notify,
            install: None,
        };
        let mut client = LspClient::spawn(&spec, Path::new("/tmp"), false).unwrap();
        assert!(client.attached(), "attach must not spawn a child process");

        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let id = client
            .request("initialize", json!({"rootUri": "file:///tmp"}))
            .unwrap();
        let request = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(request["method"], "initialize");
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
                Some(ClientEvent::ReadError(error)) => panic!("attached client error: {error}"),
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                None => panic!("the attached server never delivered the response"),
            }
        }
    }

    #[test]
    fn attach_gives_up_when_nothing_is_listening() {
        let port = free_loopback_port().unwrap();
        let attach = AttachTarget {
            host: "127.0.0.1".into(),
            port,
            env: None,
        };
        assert!(connect_attach(&attach, false).is_none());
    }

    #[test]
    fn parses_optional_server_capabilities_and_godot_triggers() {
        let result = json!({
            "capabilities": {
                "completionProvider": {"triggerCharacters": [".", "$", "\"", "ab"]},
                "signatureHelpProvider": {"triggerCharacters": ["(", ","]},
                "referencesProvider": true,
                "documentHighlightProvider": true,
                "renameProvider": {"prepareProvider": true},
            }
        });
        let caps = server_capabilities(&result, "gdscript");
        assert_eq!(caps.completion_triggers, ['.', '$', '"', '%']);
        assert!(caps.signature_help && caps.references && caps.document_highlight);
        assert!(caps.rename && caps.prepare_rename);

        let caps = server_capabilities(&result, "rust");
        assert_eq!(caps.completion_triggers, ['.', '$', '"']);

        // A boolean `renameProvider` has no prepare support; missing optional
        // providers leave everything off.
        let caps = server_capabilities(&json!({"capabilities": {"renameProvider": true}}), "rust");
        assert!(caps.rename && !caps.prepare_rename);
        assert!(!caps.signature_help && !caps.references && !caps.document_highlight);
    }

    #[test]
    fn finds_the_string_literal_around_the_caret() {
        let text = "func f():\n\tpreload(\"res://pl";
        let span = enclosing_string_span(text, text.len()).expect("unterminated literal");
        assert_eq!(&text[span.0..span.1], "\"res://pl");

        let text = "preload(\"res://a.gd\")";
        let caret = text.find("res").unwrap();
        let span = enclosing_string_span(text, caret).expect("closed literal");
        assert_eq!(&text[span.0..span.1], "\"res://a.gd\"");

        let text = "preload('res://a.gd')";
        let caret = text.find("res").unwrap();
        let span = enclosing_string_span(text, caret).expect("single quotes");
        assert_eq!(&text[span.0..span.1], "'res://a.gd'");

        let text = "var a = \"x\"; var b = \"res://b.gd\"";
        let caret = text.rfind("res").unwrap();
        let span = enclosing_string_span(text, caret).expect("second literal");
        assert_eq!(&text[span.0..span.1], "\"res://b.gd\"");

        let text = "var n = &\"res://b.gd\"";
        let caret = text.find("res").unwrap();
        let span = enclosing_string_span(text, caret).expect("StringName prefix");
        assert_eq!(&text[span.0..span.1], "\"res://b.gd\"");

        let text = "var s = \"a \\\"quoted\\\" path\"";
        let caret = text.find("path").unwrap();
        let span = enclosing_string_span(text, caret).expect("escaped quotes");
        assert_eq!(&text[span.0..span.1], "\"a \\\"quoted\\\" path\"");

        // Comments, triple-quoted strings and a caret outside a literal are not
        // resource-path completions.
        assert!(enclosing_string_span("# \"not a string", 16).is_none());
        assert!(enclosing_string_span("var x = \"\"\"res://", 17).is_none());
        assert!(enclosing_string_span("preload(\"res://a.gd\") tail", 26).is_none());
    }

    #[test]
    fn parses_prepare_and_workspace_rename_edits() {
        assert!(parse_prepare_rename(&json!(null)).is_none());
        assert_eq!(
            parse_prepare_rename(&json!({"range": {}})).as_deref(),
            Some("")
        );
        assert_eq!(
            parse_prepare_rename(&json!({"range": {}, "placeholder": "player"})).as_deref(),
            Some("player")
        );
        let edits = parse_workspace_edit(&json!({
            "changes": {
                "file:///tmp/a.gd": [{
                    "range": {"start": {"line": 1, "character": 2},
                              "end": {"line": 1, "character": 8}},
                    "newText": "hero",
                }],
            },
            "documentChanges": [{
                "textDocument": {"uri": "file:///tmp/b.gd", "version": 3},
                "edits": [{
                    "range": {"start": {"line": 0, "character": 0},
                              "end": {"line": 0, "character": 4}},
                    "newText": "hero",
                }],
            }],
        }));
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].path, PathBuf::from("/tmp/a.gd"));
        assert_eq!(edits[0].new_text, "hero");
        assert_eq!((edits[0].start_line, edits[0].start_char), (1, 2));
        assert_eq!(edits[1].path, PathBuf::from("/tmp/b.gd"));
    }

    #[test]
    fn parses_reference_locations() {
        let locations = parse_locations(&json!([
            {"uri": "file:///tmp/a.gd", "range": {"start": {"line": 2, "character": 4}}},
            {"uri": "file:///tmp/b.gd", "range": {"start": {"line": 0, "character": 0}}},
            {"range": {"start": {"line": 1, "character": 1}}},
        ]));
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0].path, PathBuf::from("/tmp/a.gd"));
        assert_eq!((locations[0].line, locations[0].character), (2, 4));
        assert_eq!(locations[1].path, PathBuf::from("/tmp/b.gd"));
    }

    #[test]
    fn parses_signature_help_labels_and_active_parameter() {
        let result = json!({
            "signatures": [{
                "label": "func clamp(value: Variant, min: Variant, max: Variant) -> Variant",
                "documentation": {"kind": "markdown", "value": "Clamp docs."},
                "parameters": [
                    {"label": "value: Variant"},
                    {"label": [27, 39]},
                ],
            }],
            "activeSignature": 4,
            "activeParameter": 1,
        });
        let help = parse_signature_help(&result).expect("signature help");
        assert_eq!(help.signatures.len(), 1);
        assert_eq!(help.active_signature, 0);
        assert_eq!(help.active_parameter, 1);
        let signature = help.active().unwrap();
        assert_eq!(
            &signature.label[help.active_span().unwrap().0..help.active_span().unwrap().1],
            "min: Variant"
        );
        assert_eq!(signature.documentation.as_deref(), Some("Clamp docs."));

        assert!(parse_signature_help(&json!(null)).is_none());
        assert!(parse_signature_help(&json!({"signatures": []})).is_none());
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
