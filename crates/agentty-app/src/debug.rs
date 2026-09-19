//! Development hooks, enabled only when `AGENTTY_DEBUG=1`: remote UI commands over the agent
//! socket and self-snapshots of Agentty's own window (no screen-recording permission needed).

#[cfg(target_os = "macos")]
use anyhow::Context as _;
use anyhow::Result;
use std::path::Path;

pub fn enabled() -> bool {
    std::env::var("AGENTTY_DEBUG").is_ok_and(|v| v == "1")
}

/// Writes a PNG of this process's frontmost on-screen window.
#[cfg(target_os = "macos")]
pub fn capture_own_window(path: &Path) -> Result<()> {
    use core_foundation::base::CFType;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::window::{create_description_from_array, create_window_list, kCGNullWindowID, kCGWindowListOptionOnScreenOnly};

    let ids = create_window_list(kCGWindowListOptionOnScreenOnly, kCGNullWindowID).context("no window list")?;
    let descriptions = create_description_from_array(ids).context("no window descriptions")?;
    let pid = std::process::id() as i64;
    let number = |dict: &CFDictionary<CFString, CFType>, key: &str| -> Option<i64> {
        dict.find(CFString::new(key)).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i64())
    };
    let window_id = descriptions
        .iter()
        // Snapshot names containing "mini" capture the floating mini panel instead of the main window.
        .find(|d| {
            number(d, "kCGWindowOwnerPID") == Some(pid)
                && (number(d, "kCGWindowLayer") == Some(0)) != path.to_string_lossy().contains("mini")
        })
        .and_then(|d| number(&d, "kCGWindowNumber"))
        .context("Agentty window not found")? as u32;
    capture_window(window_id, path)
}

/// Writes a PNG of one window (by its window number).
#[cfg(target_os = "macos")]
pub fn capture_window(window_id: u32, path: &Path) -> Result<()> {
    use core_graphics::geometry::CGRect;
    use core_graphics::window::{
        create_image, kCGWindowImageBestResolution, kCGWindowImageBoundsIgnoreFraming, kCGWindowListOptionIncludingWindow,
    };

    // SAFETY: CGRectNull is an immutable CoreGraphics constant (capture the window's own bounds).
    let bounds: CGRect = unsafe { core_graphics::display::CGRectNull };
    let image = create_image(
        bounds,
        kCGWindowListOptionIncludingWindow,
        window_id,
        kCGWindowImageBoundsIgnoreFraming | kCGWindowImageBestResolution,
    )
    .context("capture failed")?;
    let (width, height, stride) = (image.width(), image.height(), image.bytes_per_row());
    let data = image.data();
    let bytes = data.bytes();
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let row = &bytes[y * stride..y * stride + width * 4];
        for px in row.as_chunks::<4>().0 {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&rgba)?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn capture_own_window(_: &Path) -> Result<()> {
    anyhow::bail!("snapshots are only supported on macOS")
}

#[cfg(not(target_os = "macos"))]
pub fn capture_window(_: u32, _: &Path) -> Result<()> {
    anyhow::bail!("snapshots are only supported on macOS")
}

/// `\n` → CR, `\e` → ESC, `\xNN` → byte, for typing into panes.
pub fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\r'),
            Some('e') => out.push('\x1b'),
            Some('x') => {
                let hex: String = chars.by_ref().take(2).collect();
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    out.push(byte as char);
                }
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Feeds a synthetic event to an NSWindow, as if the user did it (debug test driver). Must run
/// outside any GPUI update: AppKit hands the event straight to the window.
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
pub fn synthetic_input(ns_window: crate::native::Id, command: &str, argument: &str) {
    use cocoa::base::{id, nil};
    use cocoa::foundation::{NSPoint, NSString};
    use objc::{class, msg_send, sel, sel_impl};
    let mut parts = argument.split_whitespace();
    let coords = (parts.next().and_then(|x| x.parse::<f64>().ok()), parts.next().and_then(|y| y.parse::<f64>().ok()));
    // Covered windows don't repaint, and hit testing uses the last painted frame.
    crate::native::order_front_regardless(ns_window);
    std::thread::sleep(std::time::Duration::from_millis(120));
    unsafe {
        let content: id = msg_send![ns_window, contentView];
        let frame: cocoa::foundation::NSRect = msg_send![content, frame];
        let number: isize = msg_send![ns_window, windowNumber];
        let flags = |text: &str| -> u64 {
            let mut f = 0u64;
            if text.contains("cmd") {
                f |= 1 << 20;
            }
            if text.contains("shift") {
                f |= 1 << 17;
            }
            if text.contains("ctrl") {
                f |= 1 << 18;
            }
            if text.contains("alt") {
                f |= 1 << 19;
            }
            f
        };
        let mouse = |kind: u64, x: f64, y: f64, modifiers: u64| {
            let location = NSPoint::new(x, frame.size.height - y);
            let event: id = msg_send![class!(NSEvent), mouseEventWithType: kind location: location modifierFlags: modifiers timestamp: 0.0f64 windowNumber: number context: nil eventNumber: 0isize clickCount: 1isize pressure: 1.0f32];
            let () = msg_send![ns_window, sendEvent: event];
        };
        match (command, coords) {
            ("move", (Some(x), Some(y))) => mouse(5, x, y, flags(argument)),
            // One step of a drag: the caller spaces them out so the window keeps painting.
            ("press", (Some(x), Some(y))) => {
                mouse(5, x, y, 0);
                mouse(1, x, y, 0);
            }
            ("drag-to", (Some(x), Some(y))) => mouse(6, x, y, 0),
            ("release", (Some(x), Some(y))) => mouse(2, x, y, 0),
            ("click", (Some(x), Some(y))) => {
                let right = argument.contains("right");
                let modifiers = flags(argument);
                mouse(5, x, y, 0);
                mouse(if right { 3 } else { 1 }, x, y, modifiers);
                mouse(if right { 4 } else { 2 }, x, y, modifiers);
            }
            // `scroll x y lines` (negative lines scroll up); drags come in as press/drag-to/release.
            ("scroll", (Some(x), Some(y))) => {
                let lines: i32 = argument.split_whitespace().nth(2).and_then(|v| v.parse().ok()).unwrap_or(-5);
                mouse(5, x, y, 0);
                let content_origin: NSPoint = msg_send![ns_window, convertPointToScreen: NSPoint::new(x, frame.size.height - y)];
                let screen_height = core_graphics::display::CGDisplay::main().bounds().size.height;
                #[link(name = "ApplicationServices", kind = "framework")]
                extern "C" {
                    fn CGEventCreateScrollWheelEvent2(
                        source: *const std::ffi::c_void,
                        units: u32,
                        count: u32,
                        w1: i32,
                        w2: i32,
                        w3: i32,
                    ) -> *mut std::ffi::c_void;
                    fn CGEventSetLocation(event: *mut std::ffi::c_void, location: core_graphics::geometry::CGPoint);
                    fn CFRelease(object: *const std::ffi::c_void);
                }
                // Units: 0 = pixels, 1 = lines.
                let event = CGEventCreateScrollWheelEvent2(std::ptr::null(), 1, 1, lines, 0, 0);
                if !event.is_null() {
                    CGEventSetLocation(event, core_graphics::geometry::CGPoint::new(content_origin.x, screen_height - content_origin.y));
                    let ns: id = msg_send![class!(NSEvent), eventWithCGEvent: event];
                    if ns != nil {
                        let () = msg_send![ns_window, sendEvent: ns];
                    }
                    CFRelease(event);
                }
            }
            ("key", _) | ("text", _) => {
                // Key events are only handled by the key window; this doesn't activate the app.
                let () = msg_send![ns_window, makeKeyWindow];
                let strokes: Vec<String> = if command == "text" {
                    argument.chars().map(|c| c.to_string()).collect()
                } else {
                    argument.split_whitespace().map(str::to_string).collect()
                };
                for stroke in strokes {
                    let (modifiers, key) = if command == "key" {
                        let key = stroke.rsplit('-').next().unwrap_or("").to_string();
                        (flags(&stroke), key)
                    } else {
                        (0, stroke.clone())
                    };
                    let chars = match key.as_str() {
                        "enter" => "\r".to_string(),
                        "escape" => "\u{1b}".to_string(),
                        "tab" => "\t".to_string(),
                        "backspace" => "\u{7f}".to_string(),
                        other => other.to_string(),
                    };
                    let key_code: u16 = match key.as_str() {
                        "enter" => 36,
                        "escape" => 53,
                        "tab" => 48,
                        "backspace" => 51,
                        _ => 0,
                    };
                    let text = NSString::alloc(nil).init_str(&chars);
                    for kind in [10u64, 11u64] {
                        let event: id = msg_send![class!(NSEvent), keyEventWithType: kind location: NSPoint::new(0., 0.) modifierFlags: modifiers timestamp: 0.0f64 windowNumber: number context: nil characters: text charactersIgnoringModifiers: text isARepeat: false keyCode: key_code];
                        let () = msg_send![ns_window, sendEvent: event];
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn synthetic_input(_ns_window: crate::native::Id, _command: &str, _argument: &str) {}
