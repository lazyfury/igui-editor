//! macOS bridge: Apple Events, the Dock menu, recent items and termination.
//!
//! Native callbacks only enqueue [`NativeRequest`]s and request a redraw; the
//! app loop drains them. We never replace winit's `NSApplicationDelegate`:
//! the open handler is an Apple Event target, and the optional Dock methods are
//! added to the existing delegate class with `class_addMethod`.

use std::cell::{Cell, RefCell};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::ptr;

use igui_winit::SharedWindow;
use objc2::ffi::{
    class_addMethod, class_getInstanceMethod, method_getImplementation, method_setImplementation,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, NSObject, NSObjectProtocol, Sel};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationTerminateReply, NSDocumentController, NSEventModifierFlags, NSMenu,
    NSMenuItem, NSScreen,
};
use objc2_foundation::{
    ns_string, NSAppleEventDescriptor, NSAppleEventManager, NSArray, NSRect, NSString, NSURL,
};

use crate::native::{self, NativeCommand, NativeRequest};

const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
const KEY_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

#[derive(Default)]
struct EmptyIvars;

thread_local! {
    // These hold main-thread AppKit/winit objects. They must not be dropped by
    // thread-local destructors at process exit: objc2 aborts if an autorelease
    // object is finalized after the event loop; `ManuallyDrop` leaks them for
    // the process lifetime, which is exactly what the app singleton wants.
    static WINDOW: RefCell<Option<ManuallyDrop<SharedWindow>>> = const { RefCell::new(None) };
    static OPEN_HANDLER: RefCell<Option<ManuallyDrop<Retained<OpenHandler>>>> = const { RefCell::new(None) };
    static RECENT_TARGET: RefCell<Option<ManuallyDrop<Retained<RecentTarget>>>> = const { RefCell::new(None) };
    static RECENT: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    static SET_DELEGATE_IMP: Cell<Option<Imp>> = const { Cell::new(None) };
    static MENU_TARGET: RefCell<Option<ManuallyDrop<Retained<AppMenuTarget>>>> = const { RefCell::new(None) };
    static MAIN_MENU: RefCell<Option<ManuallyDrop<Retained<NSMenu>>>> = const { RefCell::new(None) };
    /// Cursor offset from the window frame origin while a custom header drag is
    /// active. The frame is moved from the frame clock (polling the cursor),
    /// not by a nested AppKit drag loop: that loop fights winit and can swallow
    /// the release event.
    static DRAG_OFFSET: Cell<Option<(f64, f64)>> = const { Cell::new(None) };
    /// Frame saved before the custom green-button zoom, in screen coordinates.
    /// `Some` means the window is currently zoomed (maximized). We own this
    /// instead of asking winit: `Window::is_maximized` temporarily swaps a
    /// borderless window to a titled style mask, which can visibly jump.
    static ZOOM_RESTORE: Cell<Option<NSRect>> = const { Cell::new(None) };
}

fn wake() {
    WINDOW.with(|window| {
        if let Some(shared) = window.borrow().as_ref() {
            if let Some(window) = shared.borrow().as_ref() {
                window.request_redraw();
            }
        }
    });
}

/// Store the window and install the Apple Event open-documents handler.
/// Called by the host before the event loop starts.
pub fn install(window: SharedWindow) {
    WINDOW.with(|slot| *slot.borrow_mut() = Some(ManuallyDrop::new(window)));
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    OPEN_HANDLER.with(|slot| {
        if slot.borrow().is_some() {
            return;
        }
        let handler = OpenHandler::new(mtm);
        unsafe {
            let manager = NSAppleEventManager::sharedAppleEventManager();
            let _: () = msg_send![
                &manager,
                setEventHandler: &*handler,
                andSelector: sel!(handleOpenDocuments:withReplyEvent:),
                forEventClass: CORE_EVENT_CLASS,
                andEventID: OPEN_DOCUMENTS
            ];
        }
        *slot.borrow_mut() = Some(ManuallyDrop::new(handler));
    });
    install_delegate_hook(mtm);
}

/// Hook `NSApplication.setDelegate:` so winit's delegate gets our methods as
/// soon as it is installed, before cold-launch file events are dispatched.
fn install_delegate_hook(mtm: MainThreadMarker) {
    if SET_DELEGATE_IMP.with(|slot| slot.get().is_some()) {
        return;
    }
    let app = NSApplication::sharedApplication(mtm);
    let class = AnyObject::class(&app);
    unsafe {
        let method = class_getInstanceMethod(class, sel!(setDelegate:));
        if method.is_null() {
            return;
        }
        let Some(original) = method_getImplementation(method) else {
            return;
        };
        SET_DELEGATE_IMP.with(|slot| slot.set(Some(original)));
        let hook: Imp = std::mem::transmute(set_delegate_imp as *const ());
        method_setImplementation(method, hook);
    }
}

fn add_delegate_methods(class: &AnyClass) {
    let class = class as *const AnyClass as *mut AnyClass;
    unsafe {
        if !objc2::ffi::class_respondsToSelector(class, sel!(applicationDockMenu:)).as_bool() {
            let imp: Imp = std::mem::transmute(dock_menu_imp as *const ());
            class_addMethod(class, sel!(applicationDockMenu:), imp, c"@:@".as_ptr());
        }
        if !objc2::ffi::class_respondsToSelector(class, sel!(applicationShouldTerminate:)).as_bool()
        {
            let imp: Imp = std::mem::transmute(should_terminate_imp as *const ());
            class_addMethod(
                class,
                sel!(applicationShouldTerminate:),
                imp,
                c"Q@:@".as_ptr(),
            );
        }
        if !objc2::ffi::class_respondsToSelector(class, sel!(application:openURLs:)).as_bool() {
            let imp: Imp = std::mem::transmute(open_urls_imp as *const ());
            class_addMethod(class, sel!(application:openURLs:), imp, c"v@:@@".as_ptr());
        }
        if !objc2::ffi::class_respondsToSelector(
            class,
            sel!(applicationShouldHandleReopen:hasVisibleWindows:),
        )
        .as_bool()
        {
            let imp: Imp = std::mem::transmute(reopen_imp as *const ());
            class_addMethod(
                class,
                sel!(applicationShouldHandleReopen:hasVisibleWindows:),
                imp,
                c"c@:@B".as_ptr(),
            );
        }
    }
}

unsafe extern "C-unwind" fn set_delegate_imp(
    this: *mut AnyObject,
    cmd: Sel,
    delegate: *mut AnyObject,
) {
    if !delegate.is_null() {
        add_delegate_methods(AnyObject::class(unsafe { &*delegate }));
    }
    if let Some(original) = SET_DELEGATE_IMP.with(Cell::get) {
        let original: unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) =
            unsafe { std::mem::transmute(original) };
        unsafe { original(this, cmd, delegate) };
    }
}

/// Add `applicationDockMenu:` and `applicationShouldTerminate:` to the
/// existing winit delegate. Called from `EditorApp::init`, after winit has
/// installed its delegate.
pub fn install_dock() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    install_main_menu(mtm);
    let Some(delegate) = app.delegate() else {
        return;
    };
    let class = AnyObject::class(delegate.as_ref());
    add_delegate_methods(class);
}

pub fn show_window() {
    with_window(|window| window.set_visible(true));
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).activate();
    }
    wake();
}

unsafe extern "C-unwind" fn reopen_imp(
    _this: *mut AnyObject,
    _cmd: Sel,
    _sender: *mut NSApplication,
    _has_visible_windows: Bool,
) -> Bool {
    show_window();
    Bool::NO
}

pub fn set_recent(recent: &[PathBuf]) {
    RECENT.with(|slot| *slot.borrow_mut() = recent.to_vec());
}

pub fn note_recent(path: &Path) {
    if !path.exists() {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let path = NSString::from_str(&path.to_string_lossy());
    let url = NSURL::fileURLWithPath(&path);
    NSDocumentController::sharedDocumentController(mtm).noteNewRecentDocumentURL(&url);
}

/// Terminate now. The dirty-buffer confirmation already ran in the app, so this
/// only has to escape the current callback: AppKit's terminate notification
/// re-enters winit's event handler, so run it after the event callback returns.
pub fn request_quit() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    schedule(menu_target(mtm), sel!(terminateApp));
}

/// Queues a no-argument selector for the next run-loop turn. `performSelector:`
/// is how we escape AppKit callbacks without adding a block dependency.
fn schedule(target: Retained<AppMenuTarget>, selector: Sel) {
    unsafe {
        let _: () = msg_send![
            &target,
            performSelector: selector,
            withObject: Option::<&AnyObject>::None,
            afterDelay: 0.0f64
        ];
    }
}

pub fn minimize_window() {
    with_window(|window| window.set_minimized(true));
}

pub fn toggle_zoom() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(window) = key_window(mtm) else {
        return;
    };
    if let Some(frame) = ZOOM_RESTORE.with(Cell::get) {
        ZOOM_RESTORE.with(|slot| slot.set(None));
        window.setFrame_display(frame, true);
        return;
    }
    let Some(screen) = window.screen().or_else(|| NSScreen::mainScreen(mtm)) else {
        return;
    };
    let frame = window.frame();
    ZOOM_RESTORE.with(|slot| slot.set(Some(frame)));
    window.setFrame_display(screen.visibleFrame(), true);
}

/// Whether the custom header zoom/maximize is active. Used instead of
/// `winit::Window::is_maximized`, which changes the style mask of a borderless
/// window just to answer the question.
pub fn window_is_zoomed() -> bool {
    ZOOM_RESTORE.with(Cell::get).is_some()
}

fn with_window(mut action: impl FnMut(&winit::window::Window)) {
    WINDOW.with(|slot| {
        let slot = slot.borrow();
        let Some(shared) = slot.as_ref() else {
            return;
        };
        let Some(window) = shared.borrow().as_ref().cloned() else {
            return;
        };
        action(&window);
    });
}

fn key_window(mtm: MainThreadMarker) -> Option<Retained<objc2_app_kit::NSWindow>> {
    let app = NSApplication::sharedApplication(mtm);
    let window = app.keyWindow().or_else(|| app.mainWindow())?;
    // A native file dialog is a sheet and becomes the key window; window
    // commands must still target the editor underneath it.
    Some(window.sheetParent().unwrap_or(window))
}

/// Starts a custom header drag. The cursor-to-frame offset is captured here;
/// [`continue_window_drag`] then moves the frame until the button is released.
pub fn begin_window_drag() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(window) = key_window(mtm) else {
        return;
    };
    let mouse = objc2_app_kit::NSEvent::mouseLocation();
    let frame = window.frame();
    DRAG_OFFSET.with(|offset| {
        offset.set(Some((mouse.x - frame.origin.x, mouse.y - frame.origin.y)));
    });
}

pub fn continue_window_drag() {
    let Some((offset_x, offset_y)) = DRAG_OFFSET.with(Cell::get) else {
        return;
    };
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(window) = key_window(mtm) else {
        return;
    };
    let mouse = objc2_app_kit::NSEvent::mouseLocation();
    window.setFrameOrigin(objc2_foundation::NSPoint::new(
        mouse.x - offset_x,
        mouse.y - offset_y,
    ));
}

pub fn end_window_drag() {
    DRAG_OFFSET.with(|offset| offset.set(None));
}

/// Whether the left mouse button is still down. Winit can miss the release
/// event while the window is moved under the cursor; polling AppKit is the
/// reliable stop.
pub fn window_drag_button_down() -> bool {
    objc2_app_kit::NSEvent::pressedMouseButtons() & 1 != 0
}

pub fn clear_system_recent() {
    if let Some(mtm) = MainThreadMarker::new() {
        unsafe {
            NSDocumentController::sharedDocumentController(mtm).clearRecentDocuments(None);
        }
    }
}

// -- Apple Events -----------------------------------------------------------

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "IguiOpenHandler"]
    #[thread_kind = MainThreadOnly]
    #[ivars = EmptyIvars]
    struct OpenHandler;

    impl OpenHandler {
        #[unsafe(method(handleOpenDocuments:withReplyEvent:))]
        fn handle(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            let list: Option<Retained<NSAppleEventDescriptor>> = unsafe {
                msg_send![event, paramDescriptorForKeyword: KEY_DIRECT_OBJECT]
            };
            let Some(list) = list else {
                return;
            };
            let count = list.numberOfItems();
            let mut paths = Vec::new();
            for index in 1..=count {
                let Some(item) = list.descriptorAtIndex(index) else {
                    continue;
                };
                let path = item
                    .fileURLValue()
                    .and_then(|url| url.path())
                    .map(|path| path.to_string())
                    .or_else(|| item.stringValue().map(|path| path.to_string()));
                if let Some(path) = path {
                    paths.push(PathBuf::from(path));
                }
            }
            if !paths.is_empty() {
                native::push(NativeRequest::Open(paths));
                wake();
            }
        }
    }

    unsafe impl NSObjectProtocol for OpenHandler {}
);

impl OpenHandler {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(EmptyIvars);
        unsafe { msg_send![super(this), init] }
    }
}

// -- Main menu --------------------------------------------------------------

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "IguiAppMenuTarget"]
    #[thread_kind = MainThreadOnly]
    #[ivars = EmptyIvars]
    struct AppMenuTarget;

    impl AppMenuTarget {
        #[unsafe(method(performCommand:))]
        fn perform_command(&self, sender: &NSMenuItem) {
            if let Some(command) = NativeCommand::from_tag(sender.tag()) {
                native::push(NativeRequest::Command(command));
                wake();
            }
        }

        #[unsafe(method(terminateApp))]
        fn terminate_app(&self) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            NSApplication::sharedApplication(mtm).terminate(None);
        }
    }

    unsafe impl NSObjectProtocol for AppMenuTarget {}
);

impl AppMenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(EmptyIvars);
        unsafe { msg_send![super(this), init] }
    }
}

fn menu_target(mtm: MainThreadMarker) -> Retained<AppMenuTarget> {
    MENU_TARGET.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(target) = slot.as_ref() {
            return (**target).clone();
        }
        let target = AppMenuTarget::new(mtm);
        *slot = Some(ManuallyDrop::new(target.clone()));
        target
    })
}

fn command_item(
    mtm: MainThreadMarker,
    target: &AppMenuTarget,
    title: &str,
    command: NativeCommand,
    key: &str,
    modifiers: NSEventModifierFlags,
) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(title));
    item.setTag(command.tag());
    unsafe {
        item.setTarget(Some(target as &AnyObject));
        item.setAction(Some(sel!(performCommand:)));
    }
    if !key.is_empty() {
        item.setKeyEquivalent(&NSString::from_str(key));
        item.setKeyEquivalentModifierMask(modifiers);
    }
    item
}

fn standard_item(
    mtm: MainThreadMarker,
    title: &str,
    key: &str,
    modifiers: NSEventModifierFlags,
    action: Option<Sel>,
) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(title));
    if let Some(action) = action {
        unsafe { item.setAction(Some(action)) };
    }
    if !key.is_empty() {
        item.setKeyEquivalent(&NSString::from_str(key));
        item.setKeyEquivalentModifierMask(modifiers);
    }
    item
}

fn submenu_item(mtm: MainThreadMarker, title: &str, menu: &NSMenu) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(title));
    item.setSubmenu(Some(menu));
    item
}

fn build_main_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let target = menu_target(mtm);
    let app = NSApplication::sharedApplication(mtm);
    let main = NSMenu::new(mtm);
    main.setTitle(ns_string!("Main Menu"));

    let app_menu = NSMenu::new(mtm);
    app_menu.setTitle(ns_string!("igui-editor"));
    app_menu.addItem(&standard_item(
        mtm,
        "About igui-editor",
        "",
        NSEventModifierFlags::empty(),
        Some(sel!(orderFrontStandardAboutPanel:)),
    ));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    app_menu.addItem(&command_item(
        mtm,
        &target,
        "Settings…",
        NativeCommand::Settings,
        ",",
        NSEventModifierFlags::Command,
    ));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    let services_menu = NSMenu::new(mtm);
    services_menu.setTitle(ns_string!("Services"));
    app.setServicesMenu(Some(&services_menu));
    app_menu.addItem(&submenu_item(mtm, "Services", &services_menu));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    app_menu.addItem(&standard_item(
        mtm,
        "Hide igui-editor",
        "h",
        NSEventModifierFlags::Command,
        Some(sel!(hide:)),
    ));
    app_menu.addItem(&standard_item(
        mtm,
        "Hide Others",
        "h",
        NSEventModifierFlags::Command | NSEventModifierFlags::Option,
        Some(sel!(hideOtherApplications:)),
    ));
    app_menu.addItem(&standard_item(
        mtm,
        "Show All",
        "",
        NSEventModifierFlags::empty(),
        Some(sel!(unhideAllApplications:)),
    ));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    app_menu.addItem(&command_item(
        mtm,
        &target,
        "Quit igui-editor",
        NativeCommand::Quit,
        "q",
        NSEventModifierFlags::Command,
    ));
    main.addItem(&submenu_item(mtm, "igui-editor", &app_menu));

    let file_menu = NSMenu::new(mtm);
    file_menu.setTitle(ns_string!("File"));
    for item in [
        command_item(
            mtm,
            &target,
            "New Buffer",
            NativeCommand::New,
            "n",
            NSEventModifierFlags::Command,
        ),
        command_item(
            mtm,
            &target,
            "New File…",
            NativeCommand::NewFile,
            "n",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        ),
        command_item(
            mtm,
            &target,
            "Open…",
            NativeCommand::Open,
            "o",
            NSEventModifierFlags::Command,
        ),
        command_item(
            mtm,
            &target,
            "Open Project…",
            NativeCommand::OpenFolder,
            "o",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        ),
        command_item(
            mtm,
            &target,
            "Close Project",
            NativeCommand::CloseFolder,
            "",
            NSEventModifierFlags::empty(),
        ),
        NSMenuItem::separatorItem(mtm),
        command_item(
            mtm,
            &target,
            "Save",
            NativeCommand::Save,
            "s",
            NSEventModifierFlags::Command,
        ),
        command_item(
            mtm,
            &target,
            "Save As…",
            NativeCommand::SaveAs,
            "s",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        ),
        NSMenuItem::separatorItem(mtm),
        command_item(
            mtm,
            &target,
            "Close Tab",
            NativeCommand::CloseTab,
            "w",
            NSEventModifierFlags::Command,
        ),
    ] {
        file_menu.addItem(&item);
    }
    main.addItem(&submenu_item(mtm, "File", &file_menu));

    let edit_menu = NSMenu::new(mtm);
    edit_menu.setTitle(ns_string!("Edit"));
    for item in [
        command_item(
            mtm,
            &target,
            "Undo",
            NativeCommand::Undo,
            "z",
            NSEventModifierFlags::Command,
        ),
        command_item(
            mtm,
            &target,
            "Redo",
            NativeCommand::Redo,
            "z",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        ),
        NSMenuItem::separatorItem(mtm),
        command_item(
            mtm,
            &target,
            "Cut",
            NativeCommand::Cut,
            "x",
            NSEventModifierFlags::Command,
        ),
        command_item(
            mtm,
            &target,
            "Copy",
            NativeCommand::Copy,
            "c",
            NSEventModifierFlags::Command,
        ),
        command_item(
            mtm,
            &target,
            "Paste",
            NativeCommand::Paste,
            "v",
            NSEventModifierFlags::Command,
        ),
        command_item(
            mtm,
            &target,
            "Select All",
            NativeCommand::SelectAll,
            "a",
            NSEventModifierFlags::Command,
        ),
        NSMenuItem::separatorItem(mtm),
        command_item(
            mtm,
            &target,
            "Find…",
            NativeCommand::Find,
            "f",
            NSEventModifierFlags::Command,
        ),
        command_item(
            mtm,
            &target,
            "Find and Replace…",
            NativeCommand::FindReplace,
            "f",
            NSEventModifierFlags::Command | NSEventModifierFlags::Option,
        ),
        command_item(
            mtm,
            &target,
            "Find Next",
            NativeCommand::FindNext,
            "g",
            NSEventModifierFlags::Command,
        ),
        command_item(
            mtm,
            &target,
            "Find Previous",
            NativeCommand::FindPrevious,
            "g",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        ),
        command_item(
            mtm,
            &target,
            "Go to Line…",
            NativeCommand::GotoLine,
            "l",
            NSEventModifierFlags::Command,
        ),
        NSMenuItem::separatorItem(mtm),
        command_item(
            mtm,
            &target,
            "Quick Open…",
            NativeCommand::QuickOpen,
            "p",
            NSEventModifierFlags::Command,
        ),
        command_item(
            mtm,
            &target,
            "Search in Project…",
            NativeCommand::ProjectSearch,
            "f",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        ),
    ] {
        edit_menu.addItem(&item);
    }
    main.addItem(&submenu_item(mtm, "Edit", &edit_menu));

    let view_menu = NSMenu::new(mtm);
    view_menu.setTitle(ns_string!("View"));
    for item in [
        command_item(
            mtm,
            &target,
            "Sidebar",
            NativeCommand::ToggleSidebar,
            "b",
            NSEventModifierFlags::Command,
        ),
        command_item(
            mtm,
            &target,
            "Outline",
            NativeCommand::ToggleOutline,
            "o",
            NSEventModifierFlags::Command | NSEventModifierFlags::Option,
        ),
        command_item(
            mtm,
            &target,
            "Toggle Theme",
            NativeCommand::ToggleTheme,
            "l",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        ),
        NSMenuItem::separatorItem(mtm),
        command_item(
            mtm,
            &target,
            "Editor",
            NativeCommand::SetViewEdit,
            "",
            NSEventModifierFlags::empty(),
        ),
        command_item(
            mtm,
            &target,
            "Split Preview",
            NativeCommand::SetViewSplit,
            "",
            NSEventModifierFlags::empty(),
        ),
        command_item(
            mtm,
            &target,
            "Preview Only",
            NativeCommand::SetViewPreview,
            "",
            NSEventModifierFlags::empty(),
        ),
        NSMenuItem::separatorItem(mtm),
        command_item(
            mtm,
            &target,
            "Scroll Sync",
            NativeCommand::ToggleScrollSync,
            "y",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        ),
        command_item(
            mtm,
            &target,
            "Soft Wrap",
            NativeCommand::ToggleSoftWrap,
            "z",
            NSEventModifierFlags::Option,
        ),
        command_item(
            mtm,
            &target,
            "Terminal",
            NativeCommand::ToggleTerminal,
            "`",
            NSEventModifierFlags::Control,
        ),
    ] {
        view_menu.addItem(&item);
    }
    main.addItem(&submenu_item(mtm, "View", &view_menu));

    let window_menu = NSMenu::new(mtm);
    window_menu.setTitle(ns_string!("Window"));
    window_menu.addItem(&command_item(
        mtm,
        &target,
        "Minimize",
        NativeCommand::Minimize,
        "m",
        NSEventModifierFlags::Command,
    ));
    window_menu.addItem(&command_item(
        mtm,
        &target,
        "Zoom",
        NativeCommand::Zoom,
        "",
        NSEventModifierFlags::empty(),
    ));
    window_menu.addItem(&NSMenuItem::separatorItem(mtm));
    window_menu.addItem(&standard_item(
        mtm,
        "Bring All to Front",
        "",
        NSEventModifierFlags::empty(),
        Some(sel!(arrangeInFront:)),
    ));
    app.setWindowsMenu(Some(&window_menu));
    main.addItem(&submenu_item(mtm, "Window", &window_menu));

    let help_menu = NSMenu::new(mtm);
    help_menu.setTitle(ns_string!("Help"));
    app.setHelpMenu(Some(&help_menu));
    main.addItem(&submenu_item(mtm, "Help", &help_menu));

    app.setMainMenu(Some(&main));
    main
}

fn install_main_menu(mtm: MainThreadMarker) {
    MAIN_MENU.with(|slot| {
        if slot.borrow().is_some() {
            return;
        }
        let menu = build_main_menu(mtm);
        *slot.borrow_mut() = Some(ManuallyDrop::new(menu));
    });
}

// -- Dock menu --------------------------------------------------------------

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "IguiRecentTarget"]
    #[thread_kind = MainThreadOnly]
    #[ivars = EmptyIvars]
    struct RecentTarget;

    impl RecentTarget {
        #[unsafe(method(newFile:))]
        fn new_file(&self, _sender: &NSMenuItem) {
            native::push(NativeRequest::NewFile);
            wake();
        }

        #[unsafe(method(openRecent:))]
        fn open_recent(&self, sender: &NSMenuItem) {
            let Some(value) = sender.representedObject() else {
                return;
            };
            let Ok(path) = value.downcast::<NSString>() else {
                return;
            };
            native::push(NativeRequest::Open(vec![PathBuf::from(path.to_string())]));
            wake();
        }

        #[unsafe(method(clearRecent:))]
        fn clear_recent(&self, _sender: &NSMenuItem) {
            native::push(NativeRequest::ClearRecent);
            wake();
        }
    }

    unsafe impl NSObjectProtocol for RecentTarget {}
);

impl RecentTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(EmptyIvars);
        unsafe { msg_send![super(this), init] }
    }
}

fn recent_target(mtm: MainThreadMarker) -> Retained<RecentTarget> {
    RECENT_TARGET.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(target) = slot.as_ref() {
            return (**target).clone();
        }
        let target = RecentTarget::new(mtm);
        *slot = Some(ManuallyDrop::new(target.clone()));
        target
    })
}

fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Sel,
    target: &RecentTarget,
    represented: Option<&NSString>,
) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(title));
    unsafe {
        item.setTarget(Some(target as &AnyObject));
        item.setAction(Some(action));
        if let Some(value) = represented {
            item.setRepresentedObject(Some(value as &AnyObject));
        }
    }
    item
}

fn build_dock_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    let target = recent_target(mtm);

    let new_file = menu_item(mtm, "New File…", sel!(newFile:), &target, None);
    menu.addItem(&new_file);
    menu.addItem(&NSMenuItem::separatorItem(mtm));

    let recent_root = NSMenuItem::new(mtm);
    recent_root.setTitle(ns_string!("Open Recent"));
    let recent_menu = NSMenu::new(mtm);
    let recent = RECENT.with(|slot| slot.borrow().clone());
    if recent.is_empty() {
        let empty = NSMenuItem::new(mtm);
        empty.setTitle(ns_string!("No Recent Items"));
        empty.setEnabled(false);
        recent_menu.addItem(&empty);
    } else {
        for path in recent.iter().filter(|path| path.exists()) {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            let represented = NSString::from_str(&path.to_string_lossy());
            let item = menu_item(mtm, &name, sel!(openRecent:), &target, Some(&represented));
            item.setToolTip(Some(&NSString::from_str(&path.display().to_string())));
            recent_menu.addItem(&item);
        }
        if recent_menu.numberOfItems() == 0 {
            let empty = NSMenuItem::new(mtm);
            empty.setTitle(ns_string!("No Recent Items"));
            empty.setEnabled(false);
            recent_menu.addItem(&empty);
        }
        recent_menu.addItem(&NSMenuItem::separatorItem(mtm));
        let clear = menu_item(mtm, "Clear Menu", sel!(clearRecent:), &target, None);
        recent_menu.addItem(&clear);
    }
    recent_root.setSubmenu(Some(&recent_menu));
    menu.addItem(&recent_root);

    menu
}

unsafe extern "C-unwind" fn dock_menu_imp(
    _this: *mut AnyObject,
    _cmd: Sel,
    _sender: *mut NSApplication,
) -> *mut NSMenu {
    let Some(mtm) = MainThreadMarker::new() else {
        return ptr::null_mut();
    };
    Retained::autorelease_ptr(build_dock_menu(mtm))
}

unsafe extern "C-unwind" fn open_urls_imp(
    _this: *mut AnyObject,
    _cmd: Sel,
    _sender: *mut NSApplication,
    urls: *mut NSArray<NSURL>,
) {
    if urls.is_null() {
        return;
    }
    let urls = unsafe { &*urls };
    let mut paths = Vec::new();
    for url in urls.iter() {
        if let Some(path) = url.path() {
            paths.push(PathBuf::from(path.to_string()));
        }
    }
    if !paths.is_empty() {
        native::push(NativeRequest::Open(paths));
        wake();
    }
}

unsafe extern "C-unwind" fn should_terminate_imp(
    _this: *mut AnyObject,
    _cmd: Sel,
    _sender: *mut NSApplication,
) -> usize {
    // AppKit's exit path can bypass Rust destructors; kill registered
    // language-server/PTY children synchronously first.
    crate::model::processes::kill_all();
    NSApplicationTerminateReply::TerminateNow.0
}
