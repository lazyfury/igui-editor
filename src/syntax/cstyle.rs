//! C-family line tokenizer shared by Rust, C, C++, JavaScript, TypeScript, Go,
//! GDScript, JSON and TOML.
//!
//! One engine, one [`LangSpec`] per language. The carried state (`u32`) encodes
//! every multi-line construct:
//!
//! | range | meaning |
//! |---|---|
//! | `0` | neutral |
//! | `2` | backtick / template literal |
//! | `3` / `4` | `"""` / `'''` block string |
//! | `9..=263` | block comment depth (`state - 8`) |
//! | `512..=767` | Rust raw string with `state - 512` hashes |
//! | `1024` | C++ raw string with an empty delimiter |
//!
//! Delimited C++ raw strings that span lines are a documented gap (the state
//! cannot carry the delimiter); they are colored as one string on the first
//! line only.

use super::{Highlight, SyntaxProvider, TokenKind};

const S_BACKTICK: u32 = 2;
const S_TRIPLE_DQ: u32 = 3;
const S_TRIPLE_SQ: u32 = 4;
const S_BLOCK_BASE: u32 = 8;
const S_RUST_RAW_BASE: u32 = 512;
const S_CPP_RAW: u32 = 1024;

#[derive(Clone, Copy)]
pub(crate) struct StringSpec {
    pub quote: u8,
    pub escapes: bool,
}

pub(crate) struct LangSpec {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
    pub line_comment: &'static [&'static str],
    pub block_comment: Option<(&'static str, &'static str)>,
    pub nested_block: bool,
    pub strings: &'static [StringSpec],
    /// `"""` / `'''` block strings (GDScript, TOML).
    pub triple: bool,
    /// Rust raw strings, char/lifetime disambiguation and `ident!` macros.
    pub rust: bool,
    /// C++ `R"delim(... )delim"`.
    pub cpp_raw: bool,
    /// Backtick strings (Go raw strings, JS/TS template literals).
    pub template: bool,
    pub template_escapes: bool,
    /// `#...` at the start of a line (C/C++).
    pub preprocessor: bool,
    /// `@name` annotations/decorators (GDScript, TS).
    pub annotations: bool,
    /// `[table]` headers (TOML).
    pub toml_tables: bool,
    /// Uppercase-first identifiers are types.
    pub case_types: bool,
    pub keywords: &'static [&'static str],
    pub types: &'static [&'static str],
    pub constants: &'static [&'static str],
}

macro_rules! provider {
    ($ty:ident, $spec:ident) => {
        pub struct $ty;

        impl SyntaxProvider for $ty {
            fn name(&self) -> &'static str {
                $spec.name
            }

            fn extensions(&self) -> &'static [&'static str] {
                $spec.extensions
            }

            fn highlight_line(&self, line: &str, state: u32) -> (Vec<Highlight>, u32) {
                highlight(&$spec, line, state)
            }

            fn line_comment(&self) -> Option<&'static str> {
                $spec.line_comment.first().copied()
            }
        }
    };
}

/// A provider over a runtime-built [`LangSpec`] (declarative language packs).
pub(crate) struct PackProvider {
    spec: &'static LangSpec,
}

impl SyntaxProvider for PackProvider {
    fn name(&self) -> &'static str {
        self.spec.name
    }

    fn extensions(&self) -> &'static [&'static str] {
        self.spec.extensions
    }

    fn highlight_line(&self, line: &str, state: u32) -> (Vec<Highlight>, u32) {
        highlight(self.spec, line, state)
    }

    fn line_comment(&self) -> Option<&'static str> {
        self.spec.line_comment.first().copied()
    }

    fn max_line_len(&self) -> usize {
        4000
    }
}

/// Wraps a leaked spec as a provider for [`super::load_language_packs`].
pub(crate) fn provider(spec: &'static LangSpec) -> std::sync::Arc<dyn SyntaxProvider> {
    std::sync::Arc::new(PackProvider { spec })
}

fn highlight(spec: &LangSpec, line: &str, state_in: u32) -> (Vec<Highlight>, u32) {
    let mut out: Vec<Highlight> = Vec::new();
    let mut i = 0usize;
    let state = state_in;
    let len = line.len();

    if state != 0 {
        match resume(spec, line, state) {
            Resume::Closed(end, kind) => {
                push(&mut out, 0, end, kind);
                i = end;
            }
            Resume::Unterminated(next, kind) => {
                push(&mut out, 0, len, kind);
                return (out, next);
            }
        }
    }

    let mut only_ws = i == 0;
    while i < len {
        let b = line.as_bytes()[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if only_ws && spec.preprocessor && b == b'#' {
            push(&mut out, i, len, TokenKind::Macro);
            return (out, 0);
        }
        if only_ws && spec.toml_tables && b == b'[' {
            if let Some(close) = line[i..].find(']') {
                let end = i + close + 1;
                push(&mut out, i, end, TokenKind::TypeName);
                i = end;
                only_ws = false;
                continue;
            }
        }
        only_ws = false;

        if spec
            .line_comment
            .iter()
            .any(|prefix| line[i..].starts_with(prefix))
        {
            push(&mut out, i, len, TokenKind::Comment);
            break;
        }

        if let Some((open, close)) = spec.block_comment {
            if line[i..].starts_with(open) {
                let start = i;
                let body = i + open.len();
                match scan_block(line, body, 1, open, close, spec.nested_block) {
                    Some(end) => {
                        push(&mut out, start, end, TokenKind::Comment);
                        i = end;
                        continue;
                    }
                    None => {
                        let depth = remaining_depth(line, body, 1, open, close, spec.nested_block);
                        push(&mut out, start, len, TokenKind::Comment);
                        return (out, S_BLOCK_BASE + depth.clamp(1, 255));
                    }
                }
            }
        }

        if spec.rust {
            if let Some((body, hashes)) = rust_raw_start(line, i) {
                let start = i;
                match find_raw_close(line, body, hashes) {
                    Some(end) => {
                        push(&mut out, start, end, TokenKind::String);
                        i = end;
                        continue;
                    }
                    None => {
                        push(&mut out, start, len, TokenKind::String);
                        return (out, S_RUST_RAW_BASE + hashes as u32);
                    }
                }
            }
        }

        if spec.cpp_raw {
            if let Some((body, delimiter)) = cpp_raw_start(line, i) {
                let start = i;
                let close = format!("){delimiter}\"");
                match line[body..].find(&close) {
                    Some(offset) => {
                        let end = body + offset + close.len();
                        push(&mut out, start, end, TokenKind::String);
                        i = end;
                        continue;
                    }
                    None => {
                        push(&mut out, start, len, TokenKind::String);
                        return (out, if delimiter.is_empty() { S_CPP_RAW } else { 0 });
                    }
                }
            }
        }

        if spec.triple && (line[i..].starts_with("\"\"\"") || line[i..].starts_with("'''")) {
            let marker = if line[i..].starts_with("\"\"\"") {
                "\"\"\""
            } else {
                "'''"
            };
            let next_state = if marker == "\"\"\"" {
                S_TRIPLE_DQ
            } else {
                S_TRIPLE_SQ
            };
            let start = i;
            match line[i + 3..].find(marker) {
                Some(offset) => {
                    let end = i + 3 + offset + 3;
                    push(&mut out, start, end, TokenKind::String);
                    i = end;
                    continue;
                }
                None => {
                    push(&mut out, start, len, TokenKind::String);
                    return (out, next_state);
                }
            }
        }

        if spec.template && b == b'`' {
            let start = i;
            match find_template_close(line, i + 1, spec.template_escapes) {
                Some(end) => {
                    push(&mut out, start, end, TokenKind::String);
                    i = end;
                    continue;
                }
                None => {
                    push(&mut out, start, len, TokenKind::String);
                    return (out, S_BACKTICK);
                }
            }
        }

        if let Some(string) = spec.strings.iter().find(|string| string.quote == b) {
            if b == b'\'' && spec.rust {
                match rust_quote(line, i) {
                    Some((end, kind)) => {
                        push(&mut out, i, end, kind);
                        i = end;
                    }
                    None => {
                        push(&mut out, i, i + 1, TokenKind::Punctuation);
                        i += 1;
                    }
                }
                continue;
            }
            let start = i;
            match scan_quoted(line, i + 1, b, string.escapes) {
                Some(end) => {
                    push(&mut out, start, end, TokenKind::String);
                    i = end;
                    continue;
                }
                None => {
                    push(&mut out, start, len, TokenKind::String);
                    break;
                }
            }
        }

        if b.is_ascii_digit()
            || (b == b'.'
                && line
                    .as_bytes()
                    .get(i + 1)
                    .is_some_and(|next| next.is_ascii_digit()))
        {
            let end = scan_number(line, i);
            push(&mut out, i, end, TokenKind::Number);
            i = end;
            continue;
        }

        if spec.annotations
            && b == b'@'
            && line
                .as_bytes()
                .get(i + 1)
                .is_some_and(|next| next.is_ascii_alphabetic() || *next == b'_')
        {
            let mut end = i + 1;
            while end < len && is_ident_continue(line.as_bytes()[end]) {
                end += 1;
            }
            push(&mut out, i, end, TokenKind::Macro);
            i = end;
            continue;
        }

        if is_ident_start(line, i) {
            let (ident, end) = scan_ident(line, i);
            let mut kind = classify(spec, ident);
            let mut token_end = end;
            if spec.rust && kind != TokenKind::Keyword && line[end..].starts_with('!') {
                kind = TokenKind::Macro;
                token_end = end + 1;
            } else if kind == TokenKind::Plain {
                let mut look = end;
                while look < len
                    && (line.as_bytes()[look] == b' ' || line.as_bytes()[look] == b'\t')
                {
                    look += 1;
                }
                if line.as_bytes().get(look) == Some(&b'(') {
                    kind = TokenKind::Function;
                }
            }
            push(&mut out, i, token_end, kind);
            i = token_end;
            continue;
        }

        let char_len = line[i..].chars().next().map_or(1, char::len_utf8);
        push(&mut out, i, (i + char_len).min(len), TokenKind::Punctuation);
        i += char_len;
    }
    (out, 0)
}

enum Resume {
    /// A multi-line token closed on this line at `end` (exclusive, after the
    /// closing delimiter); parsing continues after it.
    Closed(usize, TokenKind),
    /// Still open; the caller colors the whole line and returns the next state.
    Unterminated(u32, TokenKind),
}

fn resume(spec: &LangSpec, line: &str, state: u32) -> Resume {
    if state == S_BACKTICK {
        return match find_template_close(line, 0, spec.template_escapes) {
            Some(end) => Resume::Closed(end, TokenKind::String),
            None => Resume::Unterminated(state, TokenKind::String),
        };
    }
    if state == S_TRIPLE_DQ || state == S_TRIPLE_SQ {
        let marker = if state == S_TRIPLE_DQ {
            "\"\"\""
        } else {
            "'''"
        };
        return match line.find(marker) {
            Some(offset) => Resume::Closed(offset + 3, TokenKind::String),
            None => Resume::Unterminated(state, TokenKind::String),
        };
    }
    if state == S_CPP_RAW {
        return match line.find(")\"") {
            Some(offset) => Resume::Closed(offset + 2, TokenKind::String),
            None => Resume::Unterminated(state, TokenKind::String),
        };
    }
    if (S_BLOCK_BASE..S_BLOCK_BASE + 256).contains(&state) {
        let Some((open, close)) = spec.block_comment else {
            return Resume::Unterminated(0, TokenKind::Plain);
        };
        let depth = state - S_BLOCK_BASE;
        return match scan_block(line, 0, depth, open, close, spec.nested_block) {
            Some(end) => Resume::Closed(end, TokenKind::Comment),
            None => Resume::Unterminated(
                S_BLOCK_BASE
                    + remaining_depth(line, 0, depth, open, close, spec.nested_block).clamp(1, 255),
                TokenKind::Comment,
            ),
        };
    }
    if (S_RUST_RAW_BASE..S_RUST_RAW_BASE + 256).contains(&state) {
        let hashes = (state - S_RUST_RAW_BASE) as usize;
        return match find_raw_close(line, 0, hashes) {
            Some(end) => Resume::Closed(end, TokenKind::String),
            None => Resume::Unterminated(state, TokenKind::String),
        };
    }
    Resume::Unterminated(0, TokenKind::Plain)
}

fn push(out: &mut Vec<Highlight>, start: usize, end: usize, kind: TokenKind) {
    if end <= start {
        return;
    }
    if let Some(last) = out.last_mut() {
        if last.kind == kind && last.end == start {
            last.end = end;
            return;
        }
    }
    out.push(Highlight { start, end, kind });
}

fn scan_block(
    line: &str,
    mut i: usize,
    mut depth: u32,
    open: &str,
    close: &str,
    nested: bool,
) -> Option<usize> {
    while i < line.len() {
        if nested && line[i..].starts_with(open) {
            depth += 1;
            i += open.len();
        } else if line[i..].starts_with(close) {
            depth = depth.saturating_sub(1);
            i += close.len();
            if depth == 0 {
                return Some(i);
            }
        } else {
            i += line[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    None
}

fn remaining_depth(
    line: &str,
    mut i: usize,
    mut depth: u32,
    open: &str,
    close: &str,
    nested: bool,
) -> u32 {
    while i < line.len() {
        if nested && line[i..].starts_with(open) {
            depth += 1;
            i += open.len();
        } else if line[i..].starts_with(close) {
            depth = depth.saturating_sub(1);
            i += close.len();
            if depth == 0 {
                return 0;
            }
        } else {
            i += line[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    depth
}

fn rust_raw_start(line: &str, i: usize) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut j = i;
    match bytes.get(j) {
        Some(b'r') => j += 1,
        Some(b'b') if bytes.get(j + 1) == Some(&b'r') => j += 2,
        _ => return None,
    }
    let mut hashes = 0usize;
    while bytes.get(j) == Some(&b'#') && hashes < 255 {
        hashes += 1;
        j += 1;
    }
    (bytes.get(j) == Some(&b'"')).then_some((j + 1, hashes))
}

fn find_raw_close(line: &str, mut i: usize, hashes: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    while let Some(offset) = line[i..].find('"') {
        let quote = i + offset;
        let mut j = quote + 1;
        let mut seen = 0;
        while seen < hashes && bytes.get(j) == Some(&b'#') {
            seen += 1;
            j += 1;
        }
        if seen == hashes {
            return Some(j);
        }
        i = quote + 1;
    }
    None
}

fn cpp_raw_start(line: &str, i: usize) -> Option<(usize, String)> {
    for prefix in ["R\"", "u8R\"", "uR\"", "UR\"", "LR\""] {
        if !line[i..].starts_with(prefix) {
            continue;
        }
        let body = i + prefix.len();
        let bytes = line.as_bytes();
        let mut j = body;
        while j < line.len() && bytes[j] != b'(' && j - body < 16 {
            let byte = bytes[j];
            if byte.is_ascii_whitespace() || byte == b'\\' || byte == b')' {
                return None;
            }
            j += 1;
        }
        if bytes.get(j) == Some(&b'(') {
            return Some((j + 1, line[body..j].to_string()));
        }
    }
    None
}

fn find_template_close(line: &str, mut i: usize, escapes: bool) -> Option<usize> {
    let bytes = line.as_bytes();
    while i < line.len() {
        if escapes && bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b'`' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

fn scan_quoted(line: &str, mut i: usize, quote: u8, escapes: bool) -> Option<usize> {
    let bytes = line.as_bytes();
    while i < line.len() {
        let byte = bytes[i];
        if escapes && byte == b'\\' {
            i += 2;
            continue;
        }
        if byte == quote {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

/// Rust `'a'` char literal vs `'a` lifetime.
fn rust_quote(line: &str, i: usize) -> Option<(usize, TokenKind)> {
    let bytes = line.as_bytes();
    let mut j = i + 1;
    let mut escaped = false;
    while j < line.len() && j - i <= 12 {
        let byte = bytes[j];
        if escaped {
            escaped = false;
            j += 1;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            j += 1;
            continue;
        }
        if byte == b'\'' {
            return Some((j + 1, TokenKind::String));
        }
        j += 1;
    }
    if bytes
        .get(i + 1)
        .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
    {
        let mut end = i + 1;
        while end < line.len() && is_ident_continue(bytes[end]) {
            end += 1;
        }
        return Some((end, TokenKind::TypeName));
    }
    None
}

fn scan_number(line: &str, start: usize) -> usize {
    let bytes = line.as_bytes();
    let mut i = start;
    while i < line.len() {
        let byte = bytes[i];
        let exponent_sign = (byte == b'+' || byte == b'-')
            && i > start
            && matches!(bytes[i - 1], b'e' | b'E' | b'p' | b'P');
        if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.' || exponent_sign {
            i += 1;
        } else {
            break;
        }
    }
    i
}

fn is_ident_start(line: &str, i: usize) -> bool {
    let byte = line.as_bytes()[i];
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$' || !byte.is_ascii()
}

fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || !byte.is_ascii()
}

fn scan_ident(line: &str, start: usize) -> (&str, usize) {
    let bytes = line.as_bytes();
    let mut end = start;
    while end < line.len() && is_ident_continue(bytes[end]) {
        end += 1;
    }
    (&line[start..end], end)
}

fn classify(spec: &LangSpec, ident: &str) -> TokenKind {
    if spec.keywords.contains(&ident) {
        return TokenKind::Keyword;
    }
    if spec.types.contains(&ident) {
        return TokenKind::TypeName;
    }
    if spec.constants.contains(&ident) {
        return TokenKind::Constant;
    }
    if spec.case_types && ident.chars().next().is_some_and(|ch| ch.is_uppercase()) {
        return TokenKind::TypeName;
    }
    TokenKind::Plain
}

// -- specs -------------------------------------------------------------------

const DQ: StringSpec = StringSpec {
    quote: b'"',
    escapes: true,
};
const SQ: StringSpec = StringSpec {
    quote: b'\'',
    escapes: true,
};
const SQ_RAW: StringSpec = StringSpec {
    quote: b'\'',
    escapes: false,
};

const STR_DQ: &[StringSpec] = &[DQ];
const STR_DQ_SQ: &[StringSpec] = &[DQ, SQ];
const STR_DQ_SQ_RAW: &[StringSpec] = &[DQ, SQ_RAW];

const RUST_KEYWORDS: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "const",
    "continue",
    "crate",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "pub",
    "ref",
    "return",
    "self",
    "Self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
    "yield",
    "macro_rules",
];
const RUST_TYPES: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32",
    "f64", "bool", "char", "str", "String", "Vec", "Option", "Result", "Box", "Arc", "Rc",
    "RefCell", "Cell", "Cow", "HashMap", "HashSet", "BTreeMap", "BTreeSet", "PathBuf", "Path",
    "VecDeque", "Pin", "Iterator", "Some", "None", "Ok", "Err", "Self",
];
const RUST_CONSTANTS: &[&str] = &["true", "false"];

const C_KEYWORDS: &[&str] = &[
    "auto", "break", "case", "const", "continue", "default", "do", "else", "enum", "extern", "for",
    "goto", "if", "inline", "register", "restrict", "return", "sizeof", "static", "struct",
    "switch", "typedef", "union", "volatile", "while", "_Bool", "_Complex", "_Atomic",
];
const C_TYPES: &[&str] = &[
    "void",
    "char",
    "short",
    "int",
    "long",
    "float",
    "double",
    "signed",
    "unsigned",
    "bool",
    "size_t",
    "ssize_t",
    "int8_t",
    "int16_t",
    "int32_t",
    "int64_t",
    "uint8_t",
    "uint16_t",
    "uint32_t",
    "uint64_t",
    "intptr_t",
    "uintptr_t",
    "ptrdiff_t",
    "FILE",
    "va_list",
    "pid_t",
    "off_t",
    "wchar_t",
];
const C_CONSTANTS: &[&str] = &["NULL", "true", "false"];

const CPP_EXTRA_KEYWORDS: &[&str] = &[
    "alignas",
    "alignof",
    "and",
    "and_eq",
    "asm",
    "bitand",
    "bitor",
    "catch",
    "class",
    "compl",
    "concept",
    "consteval",
    "constexpr",
    "constinit",
    "const_cast",
    "co_await",
    "co_return",
    "co_yield",
    "decltype",
    "delete",
    "dynamic_cast",
    "explicit",
    "export",
    "final",
    "friend",
    "import",
    "mutable",
    "namespace",
    "new",
    "noexcept",
    "not",
    "not_eq",
    "operator",
    "or",
    "or_eq",
    "override",
    "private",
    "protected",
    "public",
    "reinterpret_cast",
    "requires",
    "static_assert",
    "static_cast",
    "template",
    "this",
    "thread_local",
    "throw",
    "try",
    "typeid",
    "typename",
    "using",
    "virtual",
    "xor",
    "xor_eq",
];
const CPP_TYPES: &[&str] = &[
    "string",
    "wstring",
    "vector",
    "map",
    "unordered_map",
    "set",
    "unordered_set",
    "pair",
    "tuple",
    "array",
    "deque",
    "list",
    "optional",
    "variant",
    "any",
    "unique_ptr",
    "shared_ptr",
    "weak_ptr",
    "function",
    "string_view",
    "span",
    "size_t",
    "ptrdiff_t",
    "int8_t",
    "int16_t",
    "int32_t",
    "int64_t",
    "uint8_t",
    "uint16_t",
    "uint32_t",
    "uint64_t",
];

const JS_KEYWORDS: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "async",
    "await",
    "of",
    "static",
    "get",
    "set",
];
const JS_TYPES: &[&str] = &[
    "Array", "Object", "String", "Number", "Boolean", "Promise", "Map", "Set", "Date", "RegExp",
    "Error", "JSON", "Math", "Symbol", "BigInt", "Function",
];
const JS_CONSTANTS: &[&str] = &["true", "false", "null", "undefined", "NaN", "Infinity"];

const GO_KEYWORDS: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
];
const GO_TYPES: &[&str] = &[
    "bool",
    "byte",
    "complex64",
    "complex128",
    "error",
    "float32",
    "float64",
    "int",
    "int8",
    "int16",
    "int32",
    "int64",
    "rune",
    "string",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "uintptr",
    "any",
    "comparable",
];
const GO_CONSTANTS: &[&str] = &["true", "false", "iota", "nil"];

const GD_KEYWORDS: &[&str] = &[
    "and",
    "as",
    "assert",
    "await",
    "break",
    "breakpoint",
    "class",
    "class_name",
    "const",
    "continue",
    "elif",
    "else",
    "enum",
    "extends",
    "for",
    "func",
    "if",
    "in",
    "is",
    "match",
    "not",
    "or",
    "pass",
    "preload",
    "return",
    "self",
    "signal",
    "static",
    "super",
    "tool",
    "var",
    "while",
    "yield",
    "export",
    "onready",
];
const GD_TYPES: &[&str] = &[
    "bool",
    "int",
    "float",
    "String",
    "StringName",
    "NodePath",
    "RID",
    "Array",
    "Dictionary",
    "Callable",
    "Signal",
    "PackedByteArray",
    "PackedInt32Array",
    "PackedFloat32Array",
    "Vector2",
    "Vector2i",
    "Vector3",
    "Vector3i",
    "Vector4",
    "Rect2",
    "Transform2D",
    "Transform3D",
    "Basis",
    "Quaternion",
    "Color",
    "Node",
    "Node2D",
    "Sprite2D",
    "Control",
];
const GD_CONSTANTS: &[&str] = &["true", "false", "null", "INF", "NAN", "PI", "TAU"];

const JSON_CONSTANTS: &[&str] = &["true", "false", "null"];
const TOML_CONSTANTS: &[&str] = &["true", "false"];

pub(crate) static RUST_SPEC: LangSpec = LangSpec {
    name: "rust",
    extensions: &["rs"],
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    nested_block: true,
    strings: STR_DQ_SQ,
    triple: false,
    rust: true,
    cpp_raw: false,
    template: false,
    template_escapes: false,
    preprocessor: false,
    annotations: false,
    toml_tables: false,
    case_types: false,
    keywords: RUST_KEYWORDS,
    types: RUST_TYPES,
    constants: RUST_CONSTANTS,
};

pub(crate) static C_SPEC: LangSpec = LangSpec {
    name: "c",
    extensions: &["c", "h"],
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    nested_block: false,
    strings: STR_DQ_SQ,
    triple: false,
    rust: false,
    cpp_raw: false,
    template: false,
    template_escapes: false,
    preprocessor: true,
    annotations: false,
    toml_tables: false,
    case_types: false,
    keywords: C_KEYWORDS,
    types: C_TYPES,
    constants: C_CONSTANTS,
};

pub(crate) static CPP_SPEC: LangSpec = LangSpec {
    name: "cpp",
    extensions: &["cpp", "cc", "cxx", "hpp", "hh", "hxx"],
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    nested_block: false,
    strings: STR_DQ_SQ,
    triple: false,
    rust: false,
    cpp_raw: true,
    template: false,
    template_escapes: false,
    preprocessor: true,
    annotations: false,
    toml_tables: false,
    case_types: true,
    keywords: CPP_EXTRA_KEYWORDS,
    types: CPP_TYPES,
    constants: C_CONSTANTS,
};

pub(crate) static JS_SPEC: LangSpec = LangSpec {
    name: "javascript",
    extensions: &["js", "mjs", "cjs", "jsx"],
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    nested_block: false,
    strings: STR_DQ_SQ,
    triple: false,
    rust: false,
    cpp_raw: false,
    template: true,
    template_escapes: true,
    preprocessor: false,
    annotations: true,
    toml_tables: false,
    case_types: true,
    keywords: JS_KEYWORDS,
    types: JS_TYPES,
    constants: JS_CONSTANTS,
};

pub(crate) static TS_SPEC: LangSpec = LangSpec {
    name: "typescript",
    extensions: &["ts", "tsx", "mts", "cts"],
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    nested_block: false,
    strings: STR_DQ_SQ,
    triple: false,
    rust: false,
    cpp_raw: false,
    template: true,
    template_escapes: true,
    preprocessor: false,
    annotations: true,
    toml_tables: false,
    case_types: true,
    keywords: TS_KEYWORDS,
    types: TS_TYPES,
    constants: JS_CONSTANTS,
};

pub(crate) static GO_SPEC: LangSpec = LangSpec {
    name: "go",
    extensions: &["go"],
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    nested_block: false,
    strings: STR_DQ_SQ,
    triple: false,
    rust: false,
    cpp_raw: false,
    template: true,
    template_escapes: false,
    preprocessor: false,
    annotations: false,
    toml_tables: false,
    case_types: true,
    keywords: GO_KEYWORDS,
    types: GO_TYPES,
    constants: GO_CONSTANTS,
};

pub(crate) static GDSCRIPT_SPEC: LangSpec = LangSpec {
    name: "gdscript",
    extensions: &["gd"],
    line_comment: &["#"],
    block_comment: None,
    nested_block: false,
    strings: STR_DQ_SQ,
    triple: true,
    rust: false,
    cpp_raw: false,
    template: false,
    template_escapes: false,
    preprocessor: false,
    annotations: true,
    toml_tables: false,
    case_types: true,
    keywords: GD_KEYWORDS,
    types: GD_TYPES,
    constants: GD_CONSTANTS,
};

pub(crate) static JSON_SPEC: LangSpec = LangSpec {
    name: "json",
    extensions: &["json"],
    line_comment: &[],
    block_comment: None,
    nested_block: false,
    strings: STR_DQ,
    triple: false,
    rust: false,
    cpp_raw: false,
    template: false,
    template_escapes: false,
    preprocessor: false,
    annotations: false,
    toml_tables: false,
    case_types: false,
    keywords: &[],
    types: &[],
    constants: JSON_CONSTANTS,
};

pub(crate) static TOML_SPEC: LangSpec = LangSpec {
    name: "toml",
    extensions: &["toml"],
    line_comment: &["#"],
    block_comment: None,
    nested_block: false,
    strings: STR_DQ_SQ_RAW,
    triple: true,
    rust: false,
    cpp_raw: false,
    template: false,
    template_escapes: false,
    preprocessor: false,
    annotations: false,
    toml_tables: true,
    case_types: false,
    keywords: &[],
    types: &[],
    constants: TOML_CONSTANTS,
};

// TypeScript keywords/types are assembled from the JS lists at compile time via
// a small helper below (const arrays cannot be concatenated in stable Rust).
const TS_KEYWORDS: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "async",
    "await",
    "of",
    "static",
    "get",
    "set",
    "abstract",
    "as",
    "asserts",
    "declare",
    "enum",
    "implements",
    "infer",
    "interface",
    "is",
    "keyof",
    "namespace",
    "override",
    "private",
    "protected",
    "public",
    "readonly",
    "satisfies",
    "type",
];
const TS_TYPES: &[&str] = &[
    "Array",
    "Object",
    "String",
    "Number",
    "Boolean",
    "Promise",
    "Map",
    "Set",
    "Date",
    "RegExp",
    "Error",
    "JSON",
    "Math",
    "Symbol",
    "BigInt",
    "Function",
    "any",
    "unknown",
    "never",
    "void",
    "object",
    "symbol",
    "bigint",
    "string",
    "number",
    "boolean",
    "Record",
    "Partial",
    "Readonly",
    "Pick",
    "Omit",
    "Exclude",
    "Extract",
    "ReturnType",
];

provider!(Rust, RUST_SPEC);
provider!(C, C_SPEC);
provider!(Cpp, CPP_SPEC);
provider!(JavaScript, JS_SPEC);
provider!(TypeScript, TS_SPEC);
provider!(Go, GO_SPEC);
provider!(GdScript, GDSCRIPT_SPEC);
provider!(Json, JSON_SPEC);
provider!(Toml, TOML_SPEC);
