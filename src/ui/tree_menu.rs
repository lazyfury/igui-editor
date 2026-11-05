//! Right-click menu for a file-tree row or the Explorer's empty area.
//!
//! Rows expose file/folder actions; blank space exposes project-root create
//! actions. Same overlay pattern as the tab menu: one geometry function for
//! paint and hit testing; outside clicks / `Esc` dismiss it.

use igui_core::{Rect, Size, Vec2};
use igui_render::PaintContext;
use igui_ui::TextMeasurer;

use crate::kit;
use crate::state::{AppState, TreeMenuAction, TreeMenuUi};

const WIDTH: f32 = 240.0;
const ITEM_H: f32 = 28.0;
const PAD: f32 = 6.0;

pub fn open(state: &mut AppState, path: std::path::PathBuf, is_dir: bool, at: Vec2) {
    state.tree_menu = Some(TreeMenuUi {
        path: path.clone(),
        is_dir,
        blank: false,
        at,
        rect: Rect::ZERO,
        hits: Vec::new(),
        hover: None,
    });
    state.tab_menu = None;
    state.menu_open = None;
    state.menu_hover = None;
    state.sidebar.selected = Some(path);
    state.focus = crate::state::Focus::Explorer;
    state.dirty = true;
}

/// Opens the Explorer's empty-area menu rooted at the current project.
pub fn open_blank(state: &mut AppState, at: Vec2) {
    let Some(root) = state.ws.folder().map(std::path::Path::to_path_buf) else {
        return;
    };
    state.tree_menu = Some(TreeMenuUi {
        path: root,
        is_dir: true,
        blank: true,
        at,
        rect: Rect::ZERO,
        hits: Vec::new(),
        hover: None,
    });
    state.tab_menu = None;
    state.menu_open = None;
    state.menu_hover = None;
    state.focus = crate::state::Focus::Explorer;
    state.dirty = true;
}

const FILE_ACTIONS: [TreeMenuAction; 5] = [
    TreeMenuAction::Open,
    TreeMenuAction::Rename,
    TreeMenuAction::Delete,
    TreeMenuAction::CopyPath,
    TreeMenuAction::Reveal,
];
const FOLDER_ACTIONS: [TreeMenuAction; 7] = [
    TreeMenuAction::Open,
    TreeMenuAction::Rename,
    TreeMenuAction::NewFile,
    TreeMenuAction::NewFolder,
    TreeMenuAction::Delete,
    TreeMenuAction::CopyPath,
    TreeMenuAction::Reveal,
];
const BLANK_ACTIONS: [TreeMenuAction; 4] = [
    TreeMenuAction::NewFile,
    TreeMenuAction::NewFolder,
    TreeMenuAction::CopyPath,
    TreeMenuAction::Reveal,
];

fn actions_for(menu: &TreeMenuUi) -> &'static [TreeMenuAction] {
    if menu.blank {
        &BLANK_ACTIONS
    } else if menu.is_dir {
        &FOLDER_ACTIONS
    } else {
        &FILE_ACTIONS
    }
}

fn create_dir(menu: &TreeMenuUi) -> &std::path::Path {
    if menu.blank || menu.is_dir {
        menu.path.as_path()
    } else {
        menu.path.parent().unwrap_or(menu.path.as_path())
    }
}

fn enabled(menu: &TreeMenuUi, action: TreeMenuAction) -> bool {
    match action {
        TreeMenuAction::Open => !menu.blank && (menu.is_dir || menu.path.is_file()),
        TreeMenuAction::Rename => !menu.blank && menu.path.exists(),
        TreeMenuAction::NewFile | TreeMenuAction::NewFolder => create_dir(menu).is_dir(),
        TreeMenuAction::Delete => !menu.blank && menu.path.exists(),
        TreeMenuAction::CopyPath => true,
        TreeMenuAction::Reveal => menu.path.exists(),
    }
}

/// Menu rectangle plus `(action, rect, enabled)` rows.
pub fn layout(state: &AppState) -> (Rect, Vec<(TreeMenuAction, Rect, bool)>) {
    let Some(menu) = &state.tree_menu else {
        return (Rect::ZERO, Vec::new());
    };
    let actions = actions_for(menu);
    let viewport = state.viewport.logical_rect();
    let height = ITEM_H * actions.len() as f32 + PAD * 2.0;
    let x = if menu.at.x + WIDTH + 4.0 > viewport.right() {
        menu.at.x - WIDTH
    } else {
        menu.at.x
    };
    let y = if menu.at.y + height + 4.0 > viewport.bottom() {
        menu.at.y - height
    } else {
        menu.at.y
    };
    let rect = Rect::from_min_size(
        Vec2::new(
            x.clamp(
                viewport.left() + 4.0,
                (viewport.right() - WIDTH - 4.0).max(viewport.left() + 4.0),
            ),
            y.clamp(
                viewport.top() + 4.0,
                (viewport.bottom() - height - 4.0).max(viewport.top() + 4.0),
            ),
        ),
        Size::new(WIDTH, height),
    );
    let mut hits = Vec::with_capacity(actions.len());
    for (index, action) in actions.iter().enumerate() {
        let item = Rect::from_min_size(
            Vec2::new(rect.left(), rect.top() + PAD + index as f32 * ITEM_H),
            Size::new(WIDTH, ITEM_H),
        );
        hits.push((*action, item, enabled(menu, *action)));
    }
    (rect, hits)
}

pub fn paint(state: &mut AppState, measurer: &dyn TextMeasurer, ctx: &mut PaintContext) {
    if state.tree_menu.is_none() {
        return;
    }
    let theme = state.theme;
    let m = theme.m;
    let p = theme.p;
    let (rect, hits) = layout(state);
    kit::fill(ctx, rect, p.raised);
    ctx.stroke_rect(rect, 1.0, p.border);

    if let Some(menu) = state.tree_menu.as_mut() {
        menu.rect = rect;
        menu.hits = hits
            .iter()
            .map(|(action, rect, _)| (*action, *rect))
            .collect();
    }
    for (index, (action, item, usable)) in hits.iter().enumerate() {
        let hovered =
            state.tree_menu.as_ref().and_then(|menu| menu.hover) == Some(index) && *usable;
        if hovered {
            kit::fill(ctx, *item, p.hover);
            kit::vline(ctx, item.left(), item.top(), item.bottom(), p.accent);
        }
        kit::text(
            ctx,
            action.label(),
            item.left() + 14.0,
            kit::baseline(measurer, *item, m.font_ui),
            m.font_ui,
            if *usable { p.text } else { p.faint },
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Click {
    Outside,
    Consumed,
    Run(TreeMenuAction),
}

pub fn click(state: &mut AppState, pos: Vec2) -> Click {
    if state.tree_menu.is_none() {
        return Click::Outside;
    }
    let (rect, hits) = layout(state);
    if !rect.contains(pos) {
        return Click::Outside;
    }
    for (action, item, usable) in hits {
        if item.contains(pos) {
            return if usable {
                Click::Run(action)
            } else {
                Click::Consumed
            };
        }
    }
    Click::Consumed
}

pub fn hover_at(state: &AppState, pos: Vec2) -> Option<usize> {
    state.tree_menu.as_ref()?;
    let (rect, hits) = layout(state);
    if !rect.contains(pos) {
        return None;
    }
    hits.iter().position(|(_, item, _)| item.contains(pos))
}
