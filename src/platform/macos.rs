use std::ffi::{c_char, c_schar};
use std::path::PathBuf;

use objc2::runtime::{AnyClass, AnyObject, Imp, ProtocolObject, Sel};
use objc2::sel;
use objc2_app_kit::{NSApplication, NSApplicationDelegate};
use objc2_foundation::{MainThreadMarker, NSArray, NSURL};

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn class_addMethod(
        class: *const AnyClass,
        selector: Sel,
        implementation: Imp,
        types: *const c_char,
    ) -> c_schar;
}

unsafe extern "C" fn application_open_urls(
    _delegate: *mut AnyObject,
    _selector: Sel,
    _application: *mut AnyObject,
    urls: *mut AnyObject,
) {
    if urls.is_null() {
        return;
    }
    let urls = unsafe { &*urls.cast::<NSArray<NSURL>>() };
    let files = urls.iter().filter_map(|url| unsafe {
        if !url.isFileURL() {
            return None;
        }
        url.path().map(|path| PathBuf::from(path.to_string()))
    });
    super::enqueue_open_files(files);
}

pub fn install() {
    let Some(mtm) = MainThreadMarker::new() else {
        tracing::error!("macOS file-open handler must be installed on the main thread");
        return;
    };
    let application = NSApplication::sharedApplication(mtm);
    let Some(delegate) = (unsafe { application.delegate() }) else {
        tracing::error!("Winit did not install an NSApplication delegate");
        return;
    };
    let delegate_ptr: *const ProtocolObject<dyn NSApplicationDelegate> = &*delegate;
    let delegate_object = unsafe { &*delegate_ptr.cast::<AnyObject>() };
    let class = delegate_object.class();
    let selector = sel!(application:openURLs:);
    if class.instance_method(selector).is_some() {
        return;
    }

    let implementation = unsafe {
        std::mem::transmute::<
            unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject),
            Imp,
        >(application_open_urls)
    };
    let types = c"v@:@@";
    let added = unsafe {
        class_addMethod(
            class as *const AnyClass,
            selector,
            implementation,
            types.as_ptr(),
        )
    };
    if added == 0 {
        tracing::error!("failed to add application:openURLs: to Winit's delegate");
    }
}
