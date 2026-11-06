//! macOS helpers for the clipboard panel (#38, #39) and the history's source
//! app icons (#37): the frontmost app, giving focus back to it, synthesising
//! ⌘V, the Accessibility permission, and app icons as PNG.

use std::ffi::c_void;
use std::time::Duration;

use objc2::AnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSApplicationActivationOptions, NSBitmapImageFileType, NSBitmapImageRep, NSRunningApplication, NSWorkspace,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGEvent, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation};
use objc2_foundation::{NSDictionary, NSNumber, NSString};

/// An app the panel can paste into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontApp {
    pub pid: i32,
    pub name: String,
    pub bundle_id: String,
}

/// `kVK_ANSI_V`.
const KEY_V: u16 = 9;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
}

/// Whether Pasta may post keyboard events (Accessibility). With `prompt`,
/// macOS shows its own dialog pointing to System Settings the first time.
pub fn accessibility_trusted(prompt: bool) -> bool {
    autoreleasepool(|_| {
        if !prompt {
            return unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) };
        }
        // `kAXTrustedCheckOptionPrompt`'s value; NSDictionary is toll-free bridged to CFDictionary.
        let key = NSString::from_str("AXTrustedCheckOptionPrompt");
        let value = NSNumber::new_bool(true);
        let options: Retained<NSDictionary<NSString, AnyObject>> =
            NSDictionary::from_retained_objects(&[&*key], &[Retained::into_super(Retained::into_super(value)).into()]);
        unsafe { AXIsProcessTrustedWithOptions(Retained::as_ptr(&options).cast()) }
    })
}

/// The app in front right now, other than Pasta.
pub fn frontmost_app() -> Option<FrontApp> {
    autoreleasepool(|_| {
        let app = crate::macos::frontmost_window_pid()
            .and_then(NSRunningApplication::runningApplicationWithProcessIdentifier)
            .or_else(|| NSWorkspace::sharedWorkspace().frontmostApplication())?;
        let pid = app.processIdentifier();
        if u32::try_from(pid).ok() == Some(std::process::id()) {
            return None;
        }
        Some(FrontApp {
            pid,
            name: app.localizedName().map(|name| name.to_string()).unwrap_or_default(),
            bundle_id: app.bundleIdentifier().map(|id| id.to_string()).unwrap_or_default(),
        })
    })
}

/// Brings `pid` back to the front. False if it quit.
pub fn activate(pid: i32) -> bool {
    autoreleasepool(|_| {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .is_some_and(|app| app.activateWithOptions(NSApplicationActivationOptions::empty()))
    })
}

/// Waits (up to `timeout`) until `pid` is the frontmost app.
pub fn wait_until_front(pid: i32, timeout: Duration) -> bool {
    let started = std::time::Instant::now();
    loop {
        let front = autoreleasepool(|_| {
            NSWorkspace::sharedWorkspace().frontmostApplication().map(|app| app.processIdentifier())
        });
        if front == Some(pid) {
            return true;
        }
        if started.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

/// Posts ⌘V to the frontmost app. Needs the Accessibility permission; without
/// it macOS drops the events silently, so check [`accessibility_trusted`] first.
pub fn post_paste() -> bool {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState);
    let down = CGEvent::new_keyboard_event(source.as_deref(), KEY_V, true);
    let up = CGEvent::new_keyboard_event(source.as_deref(), KEY_V, false);
    let (Some(down), Some(up)) = (down, up) else { return false };
    CGEvent::set_flags(Some(&down), CGEventFlags::MaskCommand);
    CGEvent::set_flags(Some(&up), CGEventFlags::MaskCommand);
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&down));
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&up));
    true
}

/// The icon of the app with `bundle_id` as a PNG of about `size` points.
pub fn app_icon_png(bundle_id: &str, size: f64) -> Option<Vec<u8>> {
    if bundle_id.is_empty() {
        return None;
    }
    autoreleasepool(|_| {
        let workspace = NSWorkspace::sharedWorkspace();
        let url = workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))?;
        let path = url.path()?;
        let image = workspace.iconForFile(&path);
        let mut rect = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: size, height: size } };
        let cg_image = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
        let bitmap = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg_image);
        let png =
            unsafe { bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }?;
        Some(png.to_vec())
    })
}
