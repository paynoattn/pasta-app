//! Turns the clipboard panel's window into a non-activating `NSPanel` (#38).
//!
//! A non-activating panel takes keyboard input without making Pasta the
//! active app, so the app the user was in stays frontmost: its text cursor
//! keeps its place, and ⌘V goes to it once the panel hides (#39). It also
//! shows over full-screen apps and on every Space.
//!
//! Tauri creates an `NSWindow`; like `tauri-nspanel`, this swaps the object's
//! class for an `NSPanel` subclass that may become key, then sets the panel
//! style and behaviour. Everything runs on the main thread.

use std::sync::OnceLock;

use objc2::ffi::NSInteger;
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{ClassType, sel};
use objc2_app_kit::{NSPanel, NSWindowCollectionBehavior, NSWindowStyleMask};
use tauri::WebviewWindow;

/// Above normal and floating windows and the Dock; below the menu bar's own menus.
const PANEL_LEVEL: NSInteger = 20;

/// The `NSPanel` subclass. Tao's window class adds `padding` bytes of instance
/// variables to `NSWindow`; the subclass reserves the same, so the object keeps
/// a class of its own size (and Tao, which finds its variables by fixed offset,
/// still reads them where they are). Only an 8-byte difference is supported:
/// anything else means Tao changed, and the panel stays a normal window.
fn panel_class(padding: usize) -> Option<&'static AnyClass> {
    static CLASS: OnceLock<Option<&'static AnyClass>> = OnceLock::new();
    *CLASS.get_or_init(|| {
        extern "C-unwind" fn yes(_: &AnyObject, _: Sel) -> Bool {
            Bool::YES
        }
        extern "C-unwind" fn no(_: &AnyObject, _: Sel) -> Bool {
            Bool::NO
        }
        if padding != 0 && padding != std::mem::size_of::<usize>() {
            tracing::warn!(padding, "unexpected window layout; the clipboard panel stays a normal window");
            return None;
        }
        let Some(mut builder) = ClassBuilder::new(c"PastaClipboardPanel", NSPanel::class()) else {
            return AnyClass::get(c"PastaClipboardPanel");
        };
        if padding != 0 {
            builder.add_ivar::<usize>(c"pastaReserved");
        }
        // SAFETY: the signatures match `-(BOOL)canBecomeKeyWindow` / `canBecomeMainWindow`.
        unsafe {
            builder.add_method(sel!(canBecomeKeyWindow), yes as extern "C-unwind" fn(_, _) -> _);
            builder.add_method(sel!(canBecomeMainWindow), no as extern "C-unwind" fn(_, _) -> _);
        }
        Some(builder.register())
    })
}

/// The panel class once [`make_panel`] made it, without making it.
fn existing_panel_class() -> Option<&'static AnyClass> {
    AnyClass::get(c"PastaClipboardPanel")
}

/// Runs `f` with the window's `NSPanel` on the main thread.
fn with_panel(window: &WebviewWindow, f: impl FnOnce(&NSPanel) + Send + 'static) {
    let Ok(raw) = window.ns_window() else {
        return tracing::warn!("the panel has no native window");
    };
    let raw = raw as usize;
    let result = window.run_on_main_thread(move || {
        // SAFETY: Tauri keeps the window alive while it exists, and this runs on the main thread.
        let object: &AnyObject = unsafe { &*(raw as *const AnyObject) };
        if existing_panel_class().is_none_or(|class| object.class() != class) {
            return;
        }
        // SAFETY: the class was swapped to a subclass of NSPanel in `make_panel`.
        let panel: &NSPanel = unsafe { &*(raw as *const NSPanel) };
        f(panel);
    });
    if let Err(err) = result {
        tracing::warn!(error = %err, "couldn't reach the main thread for the panel");
    }
}

/// Converts the panel window. Call once, right after it is built.
pub fn make_panel(window: &WebviewWindow) {
    let Ok(raw) = window.ns_window() else {
        return tracing::warn!("the panel has no native window");
    };
    let raw = raw as usize;
    let result = window.run_on_main_thread(move || {
        // SAFETY: a live NSWindow on the main thread.
        let object: &AnyObject = unsafe { &*(raw as *const AnyObject) };
        let padding = object.class().instance_size().saturating_sub(NSPanel::class().instance_size());
        let Some(class) = panel_class(padding) else { return };
        if object.class().instance_size() != class.instance_size() {
            return tracing::warn!("the clipboard panel's window has an unexpected size; leaving it a normal window");
        }
        // SAFETY: same instance size (checked above), and NSPanel adds no instance
        // variables to NSWindow, which the window's current class derives from.
        unsafe { AnyObject::set_class(object, class) };
        let panel: &NSPanel = unsafe { &*(raw as *const NSPanel) };
        panel.setStyleMask(panel.styleMask() | NSWindowStyleMask::NonactivatingPanel);
        panel.setFloatingPanel(true);
        panel.setBecomesKeyOnlyIfNeeded(false);
        panel.setHidesOnDeactivate(false);
        panel.setLevel(PANEL_LEVEL);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        tracing::debug!("the clipboard panel is a non-activating panel");
    });
    if let Err(err) = result {
        tracing::warn!(error = %err, "couldn't set up the clipboard panel");
    }
}

/// Shows the panel and gives it the keyboard without activating Pasta.
pub fn show(window: &WebviewWindow) {
    with_panel(window, |panel| {
        panel.orderFrontRegardless();
        panel.makeKeyWindow();
    });
}

pub fn hide(window: &WebviewWindow) {
    with_panel(window, |panel| panel.orderOut(None));
}
