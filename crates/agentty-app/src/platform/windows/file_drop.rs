//! Windows: files dropped on a window arrive through GPUI's own `ExternalPaths` drop event (see
//! `Workbench::render`), so there is no native hook to install; the shared queue and path cleanup
//! are the same as on macOS. Ctrl+V reads copied files and images from the clipboard here.

use std::path::PathBuf;
use windows::core::w;
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

pub use crate::platform::drops::drain;

pub fn install(_ns_window: crate::native::Id) {}

const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIB: u32 = 8;

/// Files on the clipboard for Ctrl+V: copied files, and copied image data (a screenshot, an image
/// copied in a browser) written out as a PNG file. Empty when the clipboard holds text, which
/// pastes as text as before — Office copies a picture of the cells next to their text.
pub fn clipboard_paths() -> Vec<PathBuf> {
    match read() {
        Some(Copied::Files(paths)) => paths.iter().map(|p| crate::platform::drops::terminal_safe(p)).collect(),
        Some(Copied::Png(png)) => write_png(&png).into_iter().collect(),
        None => Vec::new(),
    }
}

enum Copied {
    Files(Vec<PathBuf>),
    Png(Vec<u8>),
}

fn read() -> Option<Copied> {
    // SAFETY: the clipboard is opened for this thread and closed before returning; every handle
    // read from it is used only while it is open.
    unsafe {
        OpenClipboard(None).ok()?;
        let copied = read_open();
        let _ = CloseClipboard();
        copied
    }
}

/// # Safety
/// The clipboard must be open on this thread.
unsafe fn read_open() -> Option<Copied> {
    if unsafe { IsClipboardFormatAvailable(CF_HDROP) }.is_ok() {
        let files = unsafe { with_data(CF_HDROP, |data| files(HDROP(data.as_ptr() as *mut _))) }?;
        return (!files.is_empty()).then_some(Copied::Files(files));
    }
    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) }.is_ok() {
        return None;
    }
    let png_format = unsafe { RegisterClipboardFormatW(w!("PNG")) };
    if png_format != 0 && unsafe { IsClipboardFormatAvailable(png_format) }.is_ok() {
        if let Some(png) = unsafe { with_data(png_format, |data| data.to_vec()) }.filter(|png| png.starts_with(b"\x89PNG")) {
            return Some(Copied::Png(png));
        }
    }
    if unsafe { IsClipboardFormatAvailable(CF_DIB) }.is_ok() {
        return unsafe { with_data(CF_DIB, dib_to_png) }.flatten().map(Copied::Png);
    }
    None
}

/// Runs `f` on the bytes of one clipboard format.
///
/// # Safety
/// The clipboard must be open on this thread.
unsafe fn with_data<T>(format: u32, f: impl FnOnce(&[u8]) -> T) -> Option<T> {
    let global = HGLOBAL(unsafe { GetClipboardData(format) }.ok()?.0);
    let size = unsafe { GlobalSize(global) };
    let data = unsafe { GlobalLock(global) };
    if data.is_null() {
        return None;
    }
    // SAFETY: a locked global memory block of `size` bytes, unlocked right after.
    let result = f(unsafe { std::slice::from_raw_parts(data as *const u8, size) });
    let _ = unsafe { GlobalUnlock(global) };
    Some(result)
}

/// # Safety
/// `drop` must be the clipboard's `CF_HDROP` data.
unsafe fn files(drop: HDROP) -> Vec<PathBuf> {
    let count = unsafe { DragQueryFileW(drop, u32::MAX, None) };
    (0..count.min(256))
        .filter_map(|index| {
            let length = unsafe { DragQueryFileW(drop, index, None) } as usize;
            let mut buffer = vec![0u16; length + 1];
            let written = unsafe { DragQueryFileW(drop, index, Some(&mut buffer)) } as usize;
            (written > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buffer[..written])))
        })
        .collect()
}

fn write_png(png: &[u8]) -> Option<PathBuf> {
    let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let path = crate::platform::drops::drop_dir().join(format!("pasted-{millis}.png"));
    std::fs::write(&path, png).ok().map(|()| path)
}

/// A device-independent bitmap (`CF_DIB`: a `BITMAPINFOHEADER` or a larger version of it, then
/// the pixels) as PNG. 24- and 32-bit pictures, which is what screenshots and copied images are.
fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |at: usize| Some(u32::from_le_bytes(dib.get(at..at + 4)?.try_into().ok()?));
    let header = u32_at(0)? as usize;
    let width = u32_at(4)? as i32;
    let height = u32_at(8)? as i32;
    let bits = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?);
    let compression = u32_at(16)?;
    let colors = u32_at(32)? as usize;
    if header < 40 || width <= 0 || height == 0 || width > 32_768 || height.unsigned_abs() > 32_768 || !matches!(bits, 24 | 32) {
        return None;
    }
    // BI_RGB, or BI_BITFIELDS with the usual masks (red, green, blue from the high byte down),
    // which follow a plain 40-byte header and are part of the larger ones.
    let masks = match compression {
        0 => 0,
        3 if bits == 32 => {
            if (u32_at(40)?, u32_at(44)?, u32_at(48)?) != (0xff_0000, 0xff00, 0xff) {
                return None;
            }
            if header == 40 {
                12
            } else {
                0
            }
        }
        _ => return None,
    };
    let (width, rows) = (width as usize, height.unsigned_abs() as usize);
    let stride = (width * bits as usize).div_ceil(32) * 4;
    let start = header + masks + colors * 4;
    let pixels = dib.get(start..start + stride * rows)?;
    let step = bits as usize / 8;
    let mut rgba = Vec::with_capacity(width * rows * 4);
    for row in 0..rows {
        // Bottom-up unless the height is negative.
        let source = if height > 0 { rows - 1 - row } else { row };
        for pixel in pixels[source * stride..][..width * step].chunks_exact(step) {
            rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], if step == 4 { pixel[3] } else { 255 }]);
        }
    }
    // Most programs leave the fourth byte of a 32-bit picture at 0: it is not transparency then.
    if step == 4 && rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
        rgba.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 255);
    }
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, width as u32, rows as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().ok()?.write_image_data(&rgba).ok()?;
    Some(png)
}

#[cfg(test)]
mod tests {
    use super::dib_to_png;

    fn dib(height: i32, bits: u16, pixels: &[u8]) -> Vec<u8> {
        let mut dib = Vec::new();
        for value in [40u32, 2, height as u32] {
            dib.extend_from_slice(&value.to_le_bytes());
        }
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&bits.to_le_bytes());
        dib.extend_from_slice(&[0; 24]);
        dib.extend_from_slice(pixels);
        dib
    }

    fn decoded(png: &[u8]) -> (u32, u32, Vec<u8>) {
        let mut reader = png::Decoder::new(std::io::Cursor::new(png)).read_info().unwrap();
        let mut rgba = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut rgba).unwrap();
        (info.width, info.height, rgba[..info.buffer_size()].to_vec())
    }

    #[test]
    fn a_bottom_up_24_bit_picture_turns_upright() {
        // Two by two, rows padded to 8 bytes, the bottom row first: blue, green / red, white.
        let pixels = [255, 0, 0, 0, 255, 0, 0, 0, 0, 0, 255, 255, 255, 255, 0, 0];
        let (width, height, rgba) = decoded(&dib_to_png(&dib(2, 24, &pixels)).unwrap());
        assert_eq!((width, height), (2, 2));
        assert_eq!(rgba, [255, 0, 0, 255, 255, 255, 255, 255, 0, 0, 255, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn a_32_bit_picture_without_alpha_is_opaque() {
        let pixels = [1, 2, 3, 0, 4, 5, 6, 0];
        let (_, height, rgba) = decoded(&dib_to_png(&dib(-1, 32, &pixels)).unwrap());
        assert_eq!(height, 1);
        assert_eq!(rgba, [3, 2, 1, 255, 6, 5, 4, 255]);
        // With alpha it keeps it.
        let (_, _, rgba) = decoded(&dib_to_png(&dib(-1, 32, &[1, 2, 3, 128, 4, 5, 6, 0])).unwrap());
        assert_eq!(rgba, [3, 2, 1, 128, 6, 5, 4, 0]);
    }

    #[test]
    fn short_or_odd_pictures_are_refused() {
        assert!(dib_to_png(&dib(2, 24, &[0; 8])).is_none(), "fewer pixels than the header says");
        assert!(dib_to_png(&dib(1, 8, &[0; 8])).is_none(), "a palette picture");
        assert!(dib_to_png(&[0; 10]).is_none());
    }

    /// Reads whatever is on the clipboard right now, so it only runs when asked for:
    /// `cargo test -p agentty-app clipboard_paths -- --ignored --nocapture` after copying a file
    /// or an image (Win+Shift+S, or Copy image in a browser).
    #[test]
    #[ignore = "reads the machine's clipboard"]
    fn clipboard_paths_reports_files_and_images() {
        let paths = super::clipboard_paths();
        println!("clipboard_paths: {paths:?}");
        for path in &paths {
            assert!(path.exists(), "{} does not exist", path.display());
        }
    }
}
