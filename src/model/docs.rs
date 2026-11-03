//! Renders a Godot `NativeSymbol` as a read-only editor tab.
//!
//! The view deliberately uses the normal document/tab machinery: titles,
//! scrolling, close buttons and tab switching are shared with Git views. The
//! generated text also carries byte ranges for its BBCode links so the editor
//! can turn a click on a reference into another docs view.

use std::collections::BTreeMap;

use super::bbcode::{self, DocLink, DocSpan};
use super::lsp::NativeSymbol;

#[derive(Clone, Debug, Default)]
pub struct RenderedDocs {
    pub text: String,
    /// `(start, end, link)` byte ranges in `text`.
    pub links: Vec<(usize, usize, DocLink)>,
}

pub fn render(symbol: &NativeSymbol) -> RenderedDocs {
    let mut rendered = RenderedDocs::default();
    let title = if symbol.detail.is_empty() {
        format!("{} {}", symbol.native_class, symbol.name)
    } else {
        symbol.detail.clone()
    };
    rendered.text.push_str("GODOT DOCS\n");
    rendered.text.push_str("==========\n\n");
    if !symbol.native_class.is_empty() {
        rendered.text.push_str("Class: ");
        let start = rendered.text.len();
        rendered.text.push_str(&symbol.native_class);
        rendered.links.push((
            start,
            rendered.text.len(),
            DocLink::Native {
                class: symbol.native_class.clone(),
                symbol: symbol.native_class.clone(),
            },
        ));
        rendered.text.push('\n');
        if symbol.name != symbol.native_class {
            rendered.text.push_str("Member: ");
            let start = rendered.text.len();
            rendered.text.push_str(&symbol.name);
            rendered.links.push((
                start,
                rendered.text.len(),
                DocLink::Native {
                    class: symbol.native_class.clone(),
                    symbol: symbol.name.clone(),
                },
            ));
            rendered.text.push('\n');
        }
        rendered.text.push('\n');
    }
    rendered.text.push_str(&title);
    rendered.text.push_str("\n\n");
    let spans = bbcode::parse(&symbol.documentation, &symbol.native_class);
    append_spans(&mut rendered, &spans);
    if !symbol.children.is_empty() {
        rendered.text.push_str("\n\n");
        append_children(&mut rendered, symbol);
    }
    rendered
}

fn append_spans(rendered: &mut RenderedDocs, spans: &[DocSpan]) {
    for span in spans {
        let start = rendered.text.len();
        rendered.text.push_str(&span.text);
        if let Some(link) = &span.link {
            rendered
                .links
                .push((start, rendered.text.len(), link.clone()));
        }
    }
}

fn append_children(rendered: &mut RenderedDocs, symbol: &NativeSymbol) {
    let mut groups: BTreeMap<u32, Vec<&NativeSymbol>> = BTreeMap::new();
    for child in &symbol.children {
        groups.entry(child.kind).or_default().push(child);
    }
    let order = [
        (7, "PROPERTIES"),
        (8, "FIELDS"),
        (14, "CONSTANTS"),
        (9, "CONSTRUCTORS"),
        (6, "METHODS"),
        (12, "FUNCTIONS"),
        (22, "SIGNALS"),
        (23, "ENUMS"),
    ];
    let mut first = true;
    for (kind, title) in order {
        let Some(items) = groups.remove(&kind) else {
            continue;
        };
        if !first {
            rendered.text.push('\n');
        }
        first = false;
        rendered.text.push_str(title);
        rendered.text.push_str(" (");
        rendered.text.push_str(&items.len().to_string());
        rendered.text.push_str(")\n");
        for child in items {
            append_child(rendered, symbol, child);
        }
    }
    for (kind, items) in groups {
        if !first {
            rendered.text.push('\n');
        }
        first = false;
        rendered
            .text
            .push_str(&format!("MEMBERS ({kind}) ({})\n", items.len()));
        for child in items {
            append_child(rendered, symbol, child);
        }
    }
}

fn append_child(rendered: &mut RenderedDocs, parent: &NativeSymbol, child: &NativeSymbol) {
    let label = if child.detail.is_empty() {
        child.name.clone()
    } else {
        child.detail.clone()
    };
    let start = rendered.text.len();
    rendered.text.push_str(&label);
    rendered.text.push('\n');
    rendered.links.push((
        start,
        start + label.len(),
        DocLink::Native {
            class: if child.native_class.is_empty() {
                parent.native_class.clone()
            } else {
                child.native_class.clone()
            },
            symbol: child.name.clone(),
        },
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_docs_link_back_to_their_native_class() {
        let symbol = NativeSymbol {
            name: "print".into(),
            native_class: "@GlobalScope".into(),
            detail: "func @GlobalScope.print(...) -> void".into(),
            documentation: String::new(),
            kind: 6,
            children: Vec::new(),
        };
        let rendered = render(&symbol);
        assert!(rendered.text.contains("Class: @GlobalScope"));
        assert!(rendered.links.iter().any(|(_, _, link)| {
            matches!(link, DocLink::Native { class, symbol }
                if class == "@GlobalScope" && symbol == "@GlobalScope")
        }));
    }

    #[test]
    fn renders_text_and_link_ranges() {
        let symbol = NativeSymbol {
            name: "Node".into(),
            native_class: "Node".into(),
            detail: "class Node extends Object".into(),
            documentation: "Use [method _ready] and [Node].".into(),
            kind: 5,
            children: vec![NativeSymbol {
                name: "_ready".into(),
                native_class: "Node".into(),
                detail: "func Node._ready() -> void".into(),
                documentation: String::new(),
                kind: 6,
                children: Vec::new(),
            }],
        };
        let rendered = render(&symbol);
        assert!(rendered.text.contains("GODOT DOCS"));
        assert!(rendered.text.contains("class Node extends Object"));
        assert!(rendered.text.contains("METHODS (1)"));
        assert!(rendered.links.iter().any(|(_, _, link)| {
            matches!(link, DocLink::Native { class, symbol } if class == "Node" && symbol == "Node")
        }));
        for (start, end, _) in &rendered.links {
            assert!(*start < *end && *end <= rendered.text.len());
        }
    }
}
