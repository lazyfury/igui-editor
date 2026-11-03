//! macOS bridge: Apple Events, the Dock menu, recent items and quit guard.
//!
//! Native callbacks only enqueue [`NativeRequest`]s and request a redraw; the
//! app loop drains them. We never replace winit's `NSApplicationDelegate`:
//! the open handler is an Apple Event target, and the optional Dock/quit
//! methods are added to the existing delegate class with `class_addMethod`.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::ptr;

use igui_winit::SharedWindow;
use objc2::ffi::{
    class_addMethod, class_getInstanceMethod, method_getImplementation, method_setImplementation,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, NSObject, NSObjectProtocol, Sel};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationTerminateReply, NSDocumentController, NSMenu, NSMenuItem,
};
use objc2_foundation::{
    ns_string, NSAppleEventDescriptor, NSAppleEventManager, NSArray, NSString, NSURL,
};

use crate::native::{self, NativeRequest};

const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
const KEY_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

#[derive(Default)]
struct EmptyIvars;

thread_local! {
    static WINDOW: RefCell<Option<SharedWindow>> = const { RefCell::new(None) };
    static OPEN_HANDLER: RefCell<Option<Retained<OpenHandler>>> = const { RefCell::new(None) };
    static RECENT_TARGET: RefCell<Option<Retained<RecentTarget>>> = const { RefCell::new(None) };
    static RECENT: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    static DIRTY: Cell<usize> = const { Cell::new(0) };
    static ALLOW_QUIT: Cell<bool> = const { Cell::new(false) };
    static SET_DELEGATE_IMP: Cell<Option<Imp>> = const { Cell::new(None) };
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
    WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
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
        *slot.borrow_mut() = Some(handler);
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
    let Some(delegate) = app.delegate() else {
        return;
    };
    let class = AnyObject::class(delegate.as_ref());
    add_delegate_methods(class);
}

pub fn set_recent(recent: &[PathBuf]) {
    RECENT.with(|slot| *slot.borrow_mut() = recent.to_vec());
}

pub fn set_dirty_count(dirty: usize) {
    DIRTY.with(|slot| slot.set(dirty));
}

/// Tell AppKit that the in-app quit confirmation finished.
pub fn reply_quit(terminate: bool) {
    ALLOW_QUIT.with(|slot| slot.set(terminate));
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    NSApplication::sharedApplication(mtm).replyToApplicationShouldTerminate(terminate);
}

pub fn note_recent(path: &Path) {
    if !path.is_file() {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let path = NSString::from_str(&path.to_string_lossy());
    let url = NSURL::fileURLWithPath(&path);
    NSDocumentController::sharedDocumentController(mtm).noteNewRecentDocumentURL(&url);
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
        slot.borrow_mut()
            .get_or_insert_with(|| RecentTarget::new(mtm))
            .clone()
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
        empty.setTitle(ns_string!("No Recent Files"));
        empty.setEnabled(false);
        recent_menu.addItem(&empty);
    } else {
        for path in recent.iter().filter(|path| path.is_file()) {
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
            empty.setTitle(ns_string!("No Recent Files"));
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
    let dirty = DIRTY.with(Cell::get);
    let allow = ALLOW_QUIT.with(Cell::get);
    if dirty == 0 || allow {
        return NSApplicationTerminateReply::TerminateNow.0;
    }
    native::push(NativeRequest::Quit);
    wake();
    NSApplicationTerminateReply::TerminateLater.0
}
