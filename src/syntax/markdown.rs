//! Markdown highlighting for the editor: headings, fences, block quotes, list
//! markers, inline code, emphasis markers, links, table pipes and rules.
//!
//! The carried state is just "inside a fenced code block" (`1`), so a fence
//! spanning lines colors its body as a string until the closing marker.

use super::{Highlight, SyntaxProvider, TokenKind};

/// Markdown provider.
pub struct Markdown;

const FENCE: u32 = 1;

impl SyntaxProvider for Markdown {
    fn name(&self) -> &'static str {
        "markdown"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["md", "markdown", "mdown", "mkd"]
    }

    fn highlight_line(&self, line: &str, state: u32) -> (Vec<Highlight>, u32) {
        let mut out = Vec::new();
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();

        if state == FENCE {
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                push(&mut out, indent, line.len(), TokenKind::Macro);
                return (out, 0);
            }
            push(&mut out, 0, line.len(), TokenKind::String);
            return (out, FENCE);
        }

        if trimmed.is_empty() {
            return (out, 0);
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            push(&mut out, indent, line.len(), TokenKind::Macro);
            return (out, FENCE);
        }

        let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            push(&mut out, 0, line.len(), TokenKind::Keyword);
            return (out, 0);
        }

        let rule = trimmed.trim_end();
        if rule.len() >= 3
            && (rule.chars().all(|ch| ch == '-')
                || rule.chars().all(|ch| ch == '*')
                || rule.chars().all(|ch| ch == '_'))
        {
            push(&mut out, indent, line.len(), TokenKind::Punctuation);
            return (out, 0);
        }

        let bytes = line.as_bytes();
        let mut i = indent;
        if bytes.get(i) == Some(&b'>') {
            while i < line.len() && (bytes[i] == b'>' || bytes[i] == b' ') {
                i += 1;
            }
            push(&mut out, indent, i, TokenKind::Comment);
        }

        let content = &line[i..];
        let marker_end = if content.starts_with("- ")
            || content.starts_with("* ")
            || content.starts_with("+ ")
        {
            Some(i + 2)
        } else {
            let digits = content.chars().take_while(char::is_ascii_digit).count();
            (digits > 0 && content[digits..].starts_with(". ")).then_some(i + digits + 2)
        };
        if let Some(end) = marker_end {
            push(&mut out, i, end, TokenKind::Punctuation);
            i = end;
        }

        if line[i..].starts_with("[ ]")
            || line[i..].starts_with("[x]")
            || line[i..].starts_with("[X]")
        {
            push(&mut out, i, i + 3, TokenKind::Macro);
            i += 3;
        }

        while i < line.len() {
            let byte = bytes[i];
            if byte == b'`' {
                let end = line[i + 1..]
                    .find('`')
                    .map(|offset| i + 1 + offset + 1)
                    .unwrap_or(i + 1);
                push(&mut out, i, end, TokenKind::String);
                i = end;
                continue;
            }
            if byte == b'[' {
                if let Some(close_offset) = line[i + 1..].find(']') {
                    let close = i + 1 + close_offset;
                    if line[close + 1..].starts_with('(') {
                        let url_start = close + 2;
                        if let Some(url_offset) = line[url_start..].find(')') {
                            let url_end = url_start + url_offset;
                            push(&mut out, i, i + 1, TokenKind::Punctuation);
                            push(&mut out, close, close + 2, TokenKind::Punctuation);
                            push(&mut out, url_start, url_end, TokenKind::Function);
                            push(&mut out, url_end, url_end + 1, TokenKind::Punctuation);
                            i = url_end + 1;
                            continue;
                        }
                    }
                }
            }
            let mut advanced = false;
            for marker in ["***", "___", "**", "__", "~~", "*", "_"] {
                if line[i..].starts_with(marker) {
                    push(&mut out, i, i + marker.len(), TokenKind::Macro);
                    i += marker.len();
                    advanced = true;
                    break;
                }
            }
            if advanced {
                continue;
            }
            if byte == b'|' {
                push(&mut out, i, i + 1, TokenKind::Punctuation);
                i += 1;
                continue;
            }
            i += line[i..].chars().next().map_or(1, char::len_utf8);
        }

        (out, 0)
    }
}

fn push(out: &mut Vec<Highlight>, start: usize, end: usize, kind: TokenKind) {
    let end = end.min(start.max(end));
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
