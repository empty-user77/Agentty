//! Writes frames into an H.264 MP4: a plugin's video (`media/svgsToVideo`, `media/htmlToVideo`).
//!
//! macOS encodes with AVFoundation, so nothing is installed for it. Other platforms have no encoder
//! here yet and say so.

#![allow(unexpected_cfgs)] // objc 0.2 macros check a `cargo-clippy` cfg

use std::path::Path;

/// Writes `count` frames of `width`×`height` into an MP4 at `to`, `fps` a second. `frame(i, out)`
/// fills frame `i` into `out`: BGRA, 4 bytes a pixel, rows packed, opaque.
pub fn encode_mp4(
    to: &Path,
    width: u32,
    height: u32,
    fps: u32,
    count: usize,
    mut frame: impl FnMut(usize, &mut [u8]),
) -> anyhow::Result<()> {
    if count == 0 {
        anyhow::bail!("nothing to write");
    }
    let mut writer = Mp4Writer::new(to, width, height, fps)?;
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    for index in 0..count {
        frame(index, &mut pixels);
        writer.append(&pixels)?;
    }
    writer.finish()
}

/// An MP4 being written a frame at a time (BGRA, `width`×`height`, rows packed, opaque), for frames
/// that arrive one by one (a page's snapshots).
pub struct Mp4Writer {
    inner: imp::Writer,
}

impl Mp4Writer {
    pub fn new(to: &Path, width: u32, height: u32, fps: u32) -> anyhow::Result<Self> {
        Ok(Self { inner: imp::Writer::new(to, width, height, fps)? })
    }

    pub fn append(&mut self, pixels: &[u8]) -> anyhow::Result<()> {
        self.inner.append(pixels)
    }

    pub fn finish(self) -> anyhow::Result<()> {
        self.inner.finish()
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use anyhow::{bail, Context as _};
    use block::ConcreteBlock;
    use objc::runtime::{Object, BOOL, YES};
    use objc::{class, msg_send, sel, sel_impl, Encode, Encoding};
    use std::ffi::{c_void, CString};
    use std::path::Path;
    use std::time::Duration;

    /// CoreMedia's `CMTime`, passed by value to `appendPixelBuffer:withPresentationTime:`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CMTime {
        value: i64,
        timescale: i32,
        flags: u32,
        epoch: i64,
    }

    unsafe impl Encode for CMTime {
        fn encode() -> Encoding {
            unsafe { Encoding::from_str("{?=qiIq}") }
        }
    }

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {}
    #[link(name = "CoreMedia", kind = "framework")]
    extern "C" {}
    #[link(name = "CoreVideo", kind = "framework")]
    extern "C" {
        fn CVPixelBufferPoolCreatePixelBuffer(allocator: *const c_void, pool: *mut c_void, out: *mut *mut c_void) -> i32;
        fn CVPixelBufferLockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
        fn CVPixelBufferUnlockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
        fn CVPixelBufferGetBaseAddress(buffer: *mut c_void) -> *mut u8;
        fn CVPixelBufferGetBytesPerRow(buffer: *mut c_void) -> usize;
        fn CVPixelBufferRelease(buffer: *mut c_void);
    }

    /// `kCVPixelFormatType_32BGRA`.
    const BGRA: i64 = 0x4247_5241;
    /// `AVAssetWriterStatusCompleted`.
    const COMPLETED: isize = 2;
    /// `kCMTimeFlags_Valid`.
    const VALID: u32 = 1;

    unsafe fn string(text: &str) -> *mut Object {
        let text = CString::new(text).unwrap_or_default();
        msg_send![class!(NSString), stringWithUTF8String: text.as_ptr()]
    }

    unsafe fn number(value: i64) -> *mut Object {
        msg_send![class!(NSNumber), numberWithLongLong: value]
    }

    unsafe fn dictionary(pairs: &[(&str, *mut Object)]) -> *mut Object {
        let keys: Vec<*mut Object> = pairs.iter().map(|(k, _)| string(k)).collect();
        let values: Vec<*mut Object> = pairs.iter().map(|(_, v)| *v).collect();
        msg_send![class!(NSDictionary), dictionaryWithObjects: values.as_ptr() forKeys: keys.as_ptr() count: pairs.len()]
    }

    unsafe fn describe(writer: *mut Object) -> String {
        let error: *mut Object = msg_send![writer, error];
        if error.is_null() {
            return "the video could not be written".into();
        }
        let text: *mut Object = msg_send![error, localizedDescription];
        let utf8: *const std::os::raw::c_char = msg_send![text, UTF8String];
        if utf8.is_null() {
            "the video could not be written".into()
        } else {
            std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned()
        }
    }

    pub struct Writer {
        writer: *mut Object,
        input: *mut Object,
        adaptor: *mut Object,
        width: usize,
        height: usize,
        fps: u32,
        index: i64,
    }

    impl Writer {
        pub fn new(to: &Path, width: u32, height: u32, fps: u32) -> anyhow::Result<Self> {
            if width == 0 || height == 0 || fps == 0 {
                bail!("nothing to write");
            }
            let _ = std::fs::remove_file(to);
            if let Some(folder) = to.parent() {
                std::fs::create_dir_all(folder)?;
            }
            let path = to.to_str().context("the path is not text")?;
            unsafe {
                let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: string(path)];
                let mut error: *mut Object = std::ptr::null_mut();
                let writer: *mut Object = msg_send![class!(AVAssetWriter), alloc];
                let writer: *mut Object = msg_send![writer, initWithURL: url fileType: string("public.mpeg-4") error: &mut error];
                if writer.is_null() {
                    bail!("the video file could not be made");
                }
                // A crisp picture: about 0.2 bits a pixel a frame (≈12 Mbit/s at 1080p30), and a
                // keyframe every two seconds.
                let bits = (width as f64 * height as f64 * fps as f64 * 0.2) as i64;
                let compression = dictionary(&[
                    ("AverageBitRate", number(bits)),
                    ("MaxKeyFrameInterval", number((fps * 2) as i64)),
                    ("ProfileLevel", string("H264_High_AutoLevel")),
                ]);
                let settings = dictionary(&[
                    ("AVVideoCodecKey", string("avc1")),
                    ("AVVideoWidthKey", number(width as i64)),
                    ("AVVideoHeightKey", number(height as i64)),
                    ("AVVideoCompressionPropertiesKey", compression),
                ]);
                let input: *mut Object =
                    msg_send![class!(AVAssetWriterInput), assetWriterInputWithMediaType: string("vide") outputSettings: settings];
                let () = msg_send![input, retain];
                let () = msg_send![input, setExpectsMediaDataInRealTime: false];
                let attributes =
                    dictionary(&[("PixelFormatType", number(BGRA)), ("Width", number(width as i64)), ("Height", number(height as i64))]);
                let adaptor: *mut Object = msg_send![class!(AVAssetWriterInputPixelBufferAdaptor), assetWriterInputPixelBufferAdaptorWithAssetWriterInput: input sourcePixelBufferAttributes: attributes];
                let () = msg_send![adaptor, retain];
                let () = msg_send![writer, addInput: input];
                let started: BOOL = msg_send![writer, startWriting];
                if started != YES {
                    let why = describe(writer);
                    let () = msg_send![adaptor, release];
                    let () = msg_send![input, release];
                    let () = msg_send![writer, release];
                    bail!(why);
                }
                let zero = CMTime { value: 0, timescale: fps as i32, flags: VALID, epoch: 0 };
                let () = msg_send![writer, startSessionAtSourceTime: zero];
                Ok(Self { writer, input, adaptor, width: width as usize, height: height as usize, fps, index: 0 })
            }
        }

        pub fn append(&mut self, pixels: &[u8]) -> anyhow::Result<()> {
            let (w, h) = (self.width, self.height);
            if pixels.len() < w * h * 4 {
                bail!("a frame is smaller than the video");
            }
            unsafe {
                // Not real time: wait for the encoder to take more (it says when).
                let mut waited = 0;
                loop {
                    let ready: BOOL = msg_send![self.input, isReadyForMoreMediaData];
                    if ready == YES {
                        break;
                    }
                    waited += 1;
                    if waited > 6_000 {
                        bail!("the encoder stopped taking frames");
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                let buffers: *mut c_void = msg_send![self.adaptor, pixelBufferPool];
                let mut buffer: *mut c_void = std::ptr::null_mut();
                if buffers.is_null() || CVPixelBufferPoolCreatePixelBuffer(std::ptr::null(), buffers, &mut buffer) != 0 || buffer.is_null()
                {
                    bail!("no frame buffer");
                }
                CVPixelBufferLockBaseAddress(buffer, 0);
                let base = CVPixelBufferGetBaseAddress(buffer);
                let stride = CVPixelBufferGetBytesPerRow(buffer);
                if !base.is_null() && stride >= w * 4 {
                    for row in 0..h {
                        std::ptr::copy_nonoverlapping(pixels.as_ptr().add(row * w * 4), base.add(row * stride), w * 4);
                    }
                }
                CVPixelBufferUnlockBaseAddress(buffer, 0);
                let at = CMTime { value: self.index, timescale: self.fps as i32, flags: VALID, epoch: 0 };
                let appended: BOOL = msg_send![self.adaptor, appendPixelBuffer: buffer withPresentationTime: at];
                CVPixelBufferRelease(buffer);
                if appended != YES {
                    bail!(describe(self.writer));
                }
            }
            self.index += 1;
            Ok(())
        }

        pub fn finish(self) -> anyhow::Result<()> {
            unsafe {
                let () = msg_send![self.input, markAsFinished];
                let (done, finished) = std::sync::mpsc::channel::<()>();
                let block = ConcreteBlock::new(move || {
                    let _ = done.send(());
                })
                .copy();
                let () = msg_send![self.writer, finishWritingWithCompletionHandler: &*block];
                let ended = finished.recv_timeout(Duration::from_secs(120)).is_ok();
                let status: isize = msg_send![self.writer, status];
                let why = describe(self.writer);
                let () = msg_send![self.adaptor, release];
                let () = msg_send![self.input, release];
                let () = msg_send![self.writer, release];
                if !ended || status != COMPLETED {
                    bail!(why);
                }
            }
            Ok(())
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::Path;

    pub struct Writer;

    impl Writer {
        pub fn new(_: &Path, _: u32, _: u32, _: u32) -> anyhow::Result<Self> {
            anyhow::bail!("making videos is not available on this system yet")
        }

        pub fn append(&mut self, _: &[u8]) -> anyhow::Result<()> {
            Ok(())
        }

        pub fn finish(self) -> anyhow::Result<()> {
            Ok(())
        }
    }
}
