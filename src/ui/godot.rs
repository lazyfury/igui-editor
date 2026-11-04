//! INHERITED / SIGNALS sidebar panels.
//!
//! One paint/layout function per tab records the row rectangles used by the
//! click handler, so paint and hit-testing cannot drift. Member data is read
//! from the LSP caches without cloning: a frame only formats the visible rows.

use igui_core::{Rect, Size, Vec2};
use igui_ui::PaintEnv;

use crate::kit::{self, RectExt};
use crate::state::godot::{
    native_properties, native_signals, script_properties, script_signals, ChainClass, PanelHit,
};
use crate::state::{AppState, Focus};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Inherited,
    Signals,
}

impl Mode {
    fn signals(self) -> bool {
        matches!(self, Mode::Signals)
    }
}

const PAD: f32 = 12.0;
const MEMBER_INDENT: f32 = 12.0;

pub fn paint(state: &mut AppState, env: &mut PaintEnv<'_>, rect: Rect, mode: Mode) {
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let measurer = env.measurer;
    let signals = mode.signals();
    let lsp = &state.lsp;
    let chain = &state.godot.chain;
    let panel = match mode {
        Mode::Inherited => &mut state.godot.inherited,
        Mode::Signals => &mut state.godot.signals,
    };

    panel.hits.clear();
    panel.scroll_max = 0.0;

    if chain.is_empty() {
        let text = state.godot.message.as_deref().unwrap_or("No parent class");
        kit::text(
            env.ctx,
            &kit::truncate(
                measurer,
                text,
                m.font_small,
                (rect.width() - PAD * 2.0).max(20.0),
            ),
            rect.left() + PAD,
            rect.top() + 24.0,
            m.font_small,
            p.faint,
        );
        return;
    }

    let row_h = m.row_h;
    let mut count = 0usize;
    let mut counts = vec![None; chain.len()];
    for (group, class) in chain.iter().enumerate() {
        count += 1;
        if panel.collapsed.contains(&class.name) {
            continue;
        }
        let n = class_data(lsp, class).map(|data| filtered(data, signals).count());
        counts[group] = n;
        count += n.unwrap_or(1);
    }
    panel.scroll_max = (count as f32 * row_h - rect.height()).max(0.0);
    crate::ui::scroll::clamp(&mut panel.scroll, panel.scroll_max);

    let mut y = rect.top() - panel.scroll;
    for (group, class) in chain.iter().enumerate() {
        let header = Rect::from_min_size(Vec2::new(rect.left(), y), Size::new(rect.width(), row_h));
        let collapsed = panel.collapsed.contains(&class.name);
        if header.bottom() > rect.top() && header.top() < rect.bottom() {
            kit::text(
                env.ctx,
                if collapsed { ">" } else { "v" },
                header.left() + PAD,
                kit::baseline(measurer, header, m.font_ui),
                m.font_small,
                p.faint,
            );
            let color = if class.unresolved {
                p.error
            } else if class.native {
                p.accent
            } else {
                p.text
            };
            let label = if class.unresolved {
                format!("{} (unresolved)", class.name)
            } else {
                class.name.clone()
            };
            kit::text(
                env.ctx,
                &kit::truncate(
                    measurer,
                    &label,
                    m.font_ui,
                    (header.width() - PAD * 2.0 - 26.0).max(20.0),
                ),
                header.left() + PAD + 14.0,
                kit::baseline(measurer, header, m.font_ui),
                m.font_ui,
                color,
            );
            if let Some(count) = counts[group].filter(|count| *count > 0) {
                kit::text_right(
                    env.ctx,
                    measurer,
                    &count.to_string(),
                    header.right() - PAD,
                    kit::baseline(measurer, header, m.font_small),
                    m.font_small,
                    p.faint,
                );
            }
            kit::hline(
                env.ctx,
                header.left() + PAD,
                header.right() - PAD,
                header.bottom() - 1.0,
                p.border,
            );
            panel.hits.push((header, PanelHit::Group(group)));
        }
        y += row_h;
        if collapsed {
            continue;
        }

        match class_data(lsp, class) {
            None => {
                let row =
                    Rect::from_min_size(Vec2::new(rect.left(), y), Size::new(rect.width(), row_h));
                if row.bottom() > rect.top() && row.top() < rect.bottom() {
                    let text = if class.unresolved {
                        "Run Godot once to scan this project"
                    } else {
                        "Loading…"
                    };
                    kit::text(
                        env.ctx,
                        text,
                        row.left() + PAD + MEMBER_INDENT,
                        kit::baseline(measurer, row, m.font_small),
                        m.font_small,
                        p.faint,
                    );
                }
                y += row_h;
            }
            Some(data) => {
                for (index, member) in filtered(data, signals).enumerate() {
                    let row = Rect::from_min_size(
                        Vec2::new(rect.left(), y),
                        Size::new(rect.width(), row_h),
                    );
                    if row.bottom() > rect.top() && row.top() < rect.bottom() {
                        let name_width = (row.width() - PAD * 2.0 - MEMBER_INDENT - 96.0).max(40.0);
                        kit::text(
                            env.ctx,
                            &kit::truncate(measurer, member.name(), m.font_ui, name_width),
                            row.left() + PAD + MEMBER_INDENT,
                            kit::baseline(measurer, row, m.font_ui),
                            m.font_ui,
                            p.text,
                        );
                        if !member.detail().is_empty() {
                            kit::text_right(
                                env.ctx,
                                measurer,
                                &kit::truncate(measurer, member.detail(), m.font_small, 88.0),
                                row.right() - PAD,
                                kit::baseline(measurer, row, m.font_small),
                                m.font_small,
                                p.faint,
                            );
                        }
                        panel.hits.push((row, PanelHit::Member { group, index }));
                    }
                    y += row_h;
                }
            }
        }
    }
}

pub fn scroll(state: &mut AppState, delta: f32, mode: Mode) {
    let panel = match mode {
        Mode::Inherited => &mut state.godot.inherited,
        Mode::Signals => &mut state.godot.signals,
    };
    crate::ui::scroll::apply(&mut panel.scroll, delta, panel.scroll_max);
    state.dirty = true;
}

pub fn click(state: &mut AppState, pos: Vec2, mode: Mode) -> bool {
    let signals = mode.signals();
    let hit = {
        let panel = match mode {
            Mode::Inherited => &state.godot.inherited,
            Mode::Signals => &state.godot.signals,
        };
        panel
            .hits
            .iter()
            .find(|(rect, _)| rect.contains(pos))
            .map(|(_, hit)| *hit)
    };
    let Some(hit) = hit else {
        return false;
    };
    match hit {
        PanelHit::Group(group) => {
            let Some(class) = state.godot.chain.get(group).map(|class| class.name.clone()) else {
                return false;
            };
            let panel = match mode {
                Mode::Inherited => &mut state.godot.inherited,
                Mode::Signals => &mut state.godot.signals,
            };
            if !panel.collapsed.remove(&class) {
                panel.collapsed.insert(class);
            }
            state.dirty = true;
            true
        }
        PanelHit::Member { group, index } => {
            let Some(class) = state.godot.chain.get(group).cloned() else {
                return false;
            };
            let Some(data) = class_data(&state.lsp, &class) else {
                return false;
            };
            if class.native {
                let member = filtered(data, signals)
                    .nth(index)
                    .map(|member| member.name().to_string());
                if let Some(member) = member {
                    state.open_native_docs_at(class.name, member);
                    return true;
                }
                return false;
            }
            let Some(path) = class.path else {
                return false;
            };
            let position = filtered(data, signals)
                .nth(index)
                .and_then(|member| member.position());
            let Some((line, character)) = position else {
                return false;
            };
            state.open_path(path.clone());
            if let Some(doc) = state.ws.active_doc() {
                let byte = crate::model::lsp::byte_for_position(doc, line, character);
                if let Some(doc) = state.ws.active_doc_mut() {
                    doc.set_caret(byte);
                }
            }
            state.editor.reveal = true;
            state.focus = Focus::Editor;
            state.dirty = true;
            true
        }
    }
}

enum ClassData<'a> {
    Native(&'a crate::model::lsp::NativeSymbol),
    Script(&'a [crate::model::lsp::OutlineSymbol]),
}

fn class_data<'a>(
    lsp: &'a crate::model::lsp::LspController,
    class: &ChainClass,
) -> Option<ClassData<'a>> {
    if class.unresolved {
        return None;
    }
    if class.native {
        return lsp.native_class_symbol(&class.name).map(ClassData::Native);
    }
    class
        .path
        .as_deref()
        .and_then(|path| lsp.base_symbols(path))
        .map(ClassData::Script)
}

enum Member<'a> {
    Native(&'a crate::model::lsp::NativeSymbol),
    Script(&'a crate::model::lsp::OutlineSymbol),
}

impl Member<'_> {
    fn name(&self) -> &str {
        match self {
            Member::Native(symbol) => &symbol.name,
            Member::Script(row) => &row.name,
        }
    }

    fn detail(&self) -> &str {
        match self {
            Member::Native(symbol) => &symbol.detail,
            Member::Script(row) => &row.detail,
        }
    }

    fn position(&self) -> Option<(u32, u32)> {
        match self {
            Member::Native(_) => None,
            Member::Script(row) => Some((row.line, row.character)),
        }
    }
}

fn filtered<'a>(data: ClassData<'a>, signals: bool) -> Box<dyn Iterator<Item = Member<'a>> + 'a> {
    match data {
        ClassData::Native(symbol) => {
            if signals {
                Box::new(native_signals(symbol).map(Member::Native))
            } else {
                Box::new(native_properties(symbol).map(Member::Native))
            }
        }
        ClassData::Script(rows) => {
            if signals {
                Box::new(script_signals(rows).map(Member::Script))
            } else {
                Box::new(script_properties(rows).map(Member::Script))
            }
        }
    }
}
