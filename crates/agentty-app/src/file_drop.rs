//! Dropping files onto a terminal: Finder files, screenshot thumbnails (file promises) and raw
//! image data. AppKit drag callbacks of GPUI's window class are replaced, so every external drop
//! is accepted and queued here with its window and position; the app loop hands the paths to the
//! pane under the pointer (see `Workbench::drop_files_at`).

#![allow(unexpected_cfgs)] // objc 0.2 macros check a `cargo-clippy` cfg

use objc::runtime::{Class, Object, Sel, BOOL, NO, YES};
use objc::{class, msg_send, sel, sel_impl};
use std::ffi::CStr;
use std::path::PathBuf;
use std::sync::Once;

type Id = *mut Object;

extern "C" {
    fn class_replaceMethod(
        class: *const Class,
        name: Sel,
        imp: objc::runtime::Imp,
        types: *const std::os::raw::c_char,
    ) -> Option<objc::runtime::Imp>;
}

pub use crate::platform::drops::{drain, Dropped};
use crate::platform::drops::{drop_dir, push};

fn ns_string(text: &str) -> Id {
    let c = std::ffi::CString::new(text).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

fn rust_string(value: Id) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let utf8: *const std::os::raw::c_char = unsafe { msg_send![value, UTF8String] };
    (!utf8.is_null()).then(|| unsafe { CStr::from_ptr(utf8) }.to_string_lossy().to_string())
}

const NS_DRAG_OPERATION_COPY: usize = 1;

extern "C" fn dragging_entered(_: &Object, _: Sel, _: Id) -> usize {
    NS_DRAG_OPERATION_COPY
}

extern "C" fn dragging_exited(_: &Object, _: Sel, _: Id) {}

extern "C" fn perform_drag_operation(this: &Object, _: Sel, info: Id) -> BOOL {
    unsafe {
        let window = this as *const Object as Id;
        let number: isize = msg_send![window, windowNumber];
        let content: Id = msg_send![window, contentView];
        let frame: cocoa::foundation::NSRect = msg_send![content, frame];
        let location: cocoa::foundation::NSPoint = msg_send![info, draggingLocation];
        let position = (location.x as f32, (frame.size.height - location.y) as f32);
        let pasteboard: Id = msg_send![info, draggingPasteboard];
        if crate::debug::enabled() {
            let types: Id = msg_send![pasteboard, types];
            let count: usize = if types.is_null() { 0 } else { msg_send![types, count] };
            let names: Vec<String> = (0..count).filter_map(|i| rust_string(msg_send![types, objectAtIndex: i])).collect();
            eprintln!("drop: types={names:?}");
        }

        // 1. Plain files (Finder, most apps).
        let url_class: Id = class!(NSURL) as *const Class as Id;
        let classes: Id = msg_send![class!(NSArray), arrayWithObject: url_class];
        let options: Id = msg_send![class!(NSDictionary), dictionaryWithObject: ns_string("1") forKey: ns_string("NSPasteboardURLReadingFileURLsOnlyKey")];
        let urls: Id = msg_send![pasteboard, readObjectsForClasses: classes options: options];
        let count: usize = if urls.is_null() { 0 } else { msg_send![urls, count] };
        let paths: Vec<PathBuf> = (0..count)
            .filter_map(|i| {
                let url: Id = msg_send![urls, objectAtIndex: i];
                rust_string(msg_send![url, path]).map(PathBuf::from)
            })
            .collect();
        if !paths.is_empty() {
            push(Dropped { window_number: number, position, paths });
            return YES;
        }

        // 2. File promises (e.g. the screenshot thumbnail): have them written to a temp folder.
        if let Some(receiver_class) = Class::get("NSFilePromiseReceiver") {
            let classes: Id = msg_send![class!(NSArray), arrayWithObject: receiver_class as *const Class as Id];
            let receivers: Id = msg_send![pasteboard, readObjectsForClasses: classes options: std::ptr::null_mut::<Object>()];
            let count: usize = if receivers.is_null() { 0 } else { msg_send![receivers, count] };
            if count > 0 {
                let dir: Id = msg_send![class!(NSURL), fileURLWithPath: ns_string(&drop_dir().display().to_string())];
                let queue: Id = msg_send![class!(NSOperationQueue), mainQueue];
                for i in 0..count {
                    let receiver: Id = msg_send![receivers, objectAtIndex: i];
                    let reader = block::ConcreteBlock::new(move |url: Id, error: Id| {
                        if !error.is_null() || url.is_null() {
                            return;
                        }
                        if let Some(path) = rust_string(msg_send![url, path]) {
                            push(Dropped { window_number: number, position, paths: vec![PathBuf::from(path)] });
                        }
                    })
                    .copy();
                    let empty: Id = msg_send![class!(NSDictionary), dictionary];
                    let _: () =
                        msg_send![receiver, receivePromisedFilesAtDestination: dir options: empty operationQueue: queue reader: &*reader];
                }
                return YES;
            }
        }

        // 3. Image data (copied screenshots, images dragged from browsers).
        for (kind, extension) in [("public.png", "png"), ("public.jpeg", "jpg"), ("public.tiff", "png")] {
            let mut data: Id = msg_send![pasteboard, dataForType: ns_string(kind)];
            if data.is_null() {
                continue;
            }
            if kind == "public.tiff" {
                // AI CLIs read PNG, not TIFF.
                let rep: Id = msg_send![class!(NSBitmapImageRep), imageRepWithData: data];
                let properties: Id = msg_send![class!(NSDictionary), dictionary];
                data = if rep.is_null() {
                    std::ptr::null_mut()
                } else {
                    msg_send![rep, representationUsingType: 4usize properties: properties]
                };
                if data.is_null() {
                    continue;
                }
            }
            let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
            let path = drop_dir().join(format!("dropped-{millis}.{extension}"));
            let written: BOOL = msg_send![data, writeToFile: ns_string(&path.display().to_string()) atomically: YES];
            if written == YES {
                push(Dropped { window_number: number, position, paths: vec![path] });
                return YES;
            }
        }
        NO
    }
}

/// Registers the window for file, promise and image drags (once per window) and installs the
/// drop handlers on its class (once).
pub fn install(ns_window: Id) {
    static PATCH: Once = Once::new();
    unsafe {
        let class = objc::runtime::object_getClass(ns_window);
        PATCH.call_once(|| {
            let replace = |selector: Sel, imp: objc::runtime::Imp, types: &str| {
                let types = std::ffi::CString::new(types).unwrap_or_default();
                class_replaceMethod(class, selector, imp, types.as_ptr());
            };
            replace(
                sel!(draggingEntered:),
                std::mem::transmute::<extern "C" fn(&Object, Sel, Id) -> usize, objc::runtime::Imp>(dragging_entered),
                "Q@:@",
            );
            replace(
                sel!(draggingUpdated:),
                std::mem::transmute::<extern "C" fn(&Object, Sel, Id) -> usize, objc::runtime::Imp>(dragging_entered),
                "Q@:@",
            );
            replace(
                sel!(draggingExited:),
                std::mem::transmute::<extern "C" fn(&Object, Sel, Id), objc::runtime::Imp>(dragging_exited),
                "v@:@",
            );
            replace(
                sel!(performDragOperation:),
                std::mem::transmute::<extern "C" fn(&Object, Sel, Id) -> BOOL, objc::runtime::Imp>(perform_drag_operation),
                "c@:@",
            );
        });
        let types: Id = msg_send![class!(NSMutableArray), array];
        for kind in ["NSFilenamesPboardType", "public.file-url", "public.png", "public.tiff", "public.jpeg"] {
            let _: () = msg_send![types, addObject: ns_string(kind)];
        }
        if let Some(receiver_class) = Class::get("NSFilePromiseReceiver") {
            let promised: Id = msg_send![receiver_class, readableDraggedTypes];
            if !promised.is_null() {
                let _: () = msg_send![types, addObjectsFromArray: promised];
            }
        }
        let _: () = msg_send![ns_window, registerForDraggedTypes: types];
    }
}
