use std::ffi::{c_char, c_schar};
use std::path::PathBuf;
use std::process::Command;

use objc2::runtime::{AnyClass, AnyObject, Imp, ProtocolObject, Sel};
use objc2::sel;
use objc2_app_kit::{NSApplication, NSApplicationDelegate, NSMenuItem};
use objc2_foundation::{NSArray, MainThreadMarker, NSString, NSURL};

use crate::config;

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

unsafe extern "C" fn open_settings_file(
    _delegate: *mut AnyObject,
    _selector: Sel,
    _sender: *mut AnyObject,
) {
    let path = match config::ensure_default_config_file() {
        Ok(path) => path,
        Err(error) => {
            tracing::error!(%error, "failed to prepare Mado settings file");
            return;
        }
    };

    if let Err(error) = Command::new("open").arg(&path).spawn() {
        tracing::error!(path = %path.display(), %error, "failed to open Mado settings file");
    }
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
    install_delegate_method(
        class,
        sel!(application:openURLs:),
        unsafe {
            std::mem::transmute::<
                unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject),
                Imp,
            >(application_open_urls)
        },
        c"v@:@@",
        "application:openURLs:",
    );
    install_delegate_method(
        class,
        sel!(openSettings:),
        unsafe {
            std::mem::transmute::<unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject), Imp>(
                open_settings_file,
            )
        },
        c"v@:@",
        "openSettings:",
    );
}

fn install_delegate_method(
    class: &AnyClass,
    selector: Sel,
    implementation: Imp,
    types: &std::ffi::CStr,
    name: &str,
) {
    if class.instance_method(selector).is_some() {
        return;
    }

    let added = unsafe {
        class_addMethod(
            class as *const AnyClass,
            selector,
            implementation,
            types.as_ptr(),
        )
    };
    if added == 0 {
        tracing::error!(method = name, "failed to add method to Winit delegate");
    }
}

pub fn install_native_menu() {
    let Some(mtm) = MainThreadMarker::new() else {
        tracing::error!("macOS menu must be installed on the main thread");
        return;
    };
    let application = NSApplication::sharedApplication(mtm);
    let Some(delegate) = (unsafe { application.delegate() }) else {
        tracing::error!("Winit did not install an NSApplication delegate");
        return;
    };
    let delegate_ptr: *const ProtocolObject<dyn NSApplicationDelegate> = &*delegate;
    let delegate_object = unsafe { &*delegate_ptr.cast::<AnyObject>() };
    let Some(main_menu) = (unsafe { application.mainMenu() }) else {
        tracing::warn!("main menu was not available yet");
        return;
    };
    let Some(app_menu_item) = (unsafe { main_menu.itemAtIndex(0) }) else {
        tracing::warn!("application menu item was not available");
        return;
    };
    let Some(app_menu) = (unsafe { app_menu_item.submenu() }) else {
        tracing::warn!("application submenu was not available");
        return;
    };

    let settings_title = NSString::from_str("Settings...");
    if unsafe { app_menu.indexOfItemWithTitle(&settings_title) } >= 0 {
        return;
    }

    let settings_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &settings_title,
            Some(sel!(openSettings:)),
            &NSString::from_str(","),
        )
    };
    unsafe {
        settings_item.setTarget(Some(delegate_object));
        app_menu.insertItem_atIndex(&settings_item, 1);
    }
}
