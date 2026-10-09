//! Records the microphone into a WAV file: the status bar's voice input.
//!
//! macOS records with AVFoundation (`AVAudioRecorder`), so nothing is installed for it, and asks
//! for the microphone through the system prompt the app's `NSMicrophoneUsageDescription` words.
//! Other platforms have no recorder here yet; `HAS_MIC` hides the button there.

#![allow(unexpected_cfgs)] // objc 0.2 macros check a `cargo-clippy` cfg

use std::path::Path;

/// Whether this platform can record a voice prompt in the app.
pub const HAS_MIC: bool = cfg!(target_os = "macos");

/// Whether the app may use the microphone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    /// Not asked yet: recording asks first.
    Undetermined,
    Granted,
    /// Turned off in System Settings (or blocked by a profile).
    Denied,
}

pub fn permission() -> Permission {
    imp::permission()
}

/// Asks for the microphone with the system prompt; `done(granted)` runs on some other thread once
/// the person answers (at once when they already did).
pub fn request_permission(done: impl FnOnce(bool) + Send + 'static) {
    imp::request_permission(done)
}

/// Opens the microphone page of System Settings, where a denied permission is turned back on.
pub fn open_privacy_settings() {
    imp::open_privacy_settings()
}

/// A recording in progress: 16 kHz mono 16-bit PCM into a WAV file. Dropping it stops it.
pub struct Recorder {
    inner: imp::Recorder,
}

impl Recorder {
    /// Starts recording into `to` (replaced if it exists). Fails without a microphone or permission.
    pub fn start(to: &Path) -> anyhow::Result<Self> {
        Ok(Self { inner: imp::Recorder::start(to)? })
    }

    /// Stops and closes the file, which is then complete on disk.
    pub fn stop(self) {
        drop(self)
    }

    /// Recent input level, 0.0 (silence) to 1.0, for the button to show it hears something.
    pub fn level(&self) -> f32 {
        self.inner.level()
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::Permission;
    use anyhow::{bail, Context as _};
    use block::ConcreteBlock;
    use objc::runtime::{Object, BOOL, NO, YES};
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::{CStr, CString};
    use std::path::Path;
    use std::sync::Mutex;

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        static AVMediaTypeAudio: *mut Object;
        static AVFormatIDKey: *mut Object;
        static AVSampleRateKey: *mut Object;
        static AVNumberOfChannelsKey: *mut Object;
        static AVLinearPCMBitDepthKey: *mut Object;
        static AVLinearPCMIsFloatKey: *mut Object;
        static AVLinearPCMIsBigEndianKey: *mut Object;
    }

    /// `kAudioFormatLinearPCM` ('lpcm').
    const LINEAR_PCM: u32 = 0x6C70_636D;

    pub fn permission() -> Permission {
        // AVAuthorizationStatus: 0 not determined, 1 restricted, 2 denied, 3 authorized.
        let status: isize = unsafe { msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: AVMediaTypeAudio] };
        match status {
            3 => Permission::Granted,
            0 => Permission::Undetermined,
            _ => Permission::Denied,
        }
    }

    pub fn request_permission(done: impl FnOnce(bool) + Send + 'static) {
        // The block may only be called once, but its type is `Fn`: hand the closure over through
        // a slot it is taken out of.
        let slot = Mutex::new(Some(done));
        let block = ConcreteBlock::new(move |granted: BOOL| {
            if let Some(done) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
                done(granted != NO);
            }
        })
        .copy();
        unsafe {
            let _: () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType: AVMediaTypeAudio completionHandler: &*block];
        }
    }

    pub fn open_privacy_settings() {
        let _ = std::process::Command::new("/usr/bin/open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
            .spawn();
    }

    unsafe fn string(text: &str) -> *mut Object {
        let text = CString::new(text).unwrap_or_default();
        msg_send![class!(NSString), stringWithUTF8String: text.as_ptr()]
    }

    pub struct Recorder {
        recorder: *mut Object,
    }

    impl Recorder {
        pub fn start(to: &Path) -> anyhow::Result<Self> {
            let path = to.to_str().context("the recording path is not UTF-8")?;
            let _ = std::fs::remove_file(to);
            unsafe {
                let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
                let result = Self::start_in_pool(path);
                let _: () = msg_send![pool, drain];
                result
            }
        }

        unsafe fn start_in_pool(path: &str) -> anyhow::Result<Self> {
            let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: string(path)];
            let keys = [
                AVFormatIDKey,
                AVSampleRateKey,
                AVNumberOfChannelsKey,
                AVLinearPCMBitDepthKey,
                AVLinearPCMIsFloatKey,
                AVLinearPCMIsBigEndianKey,
            ];
            let values: [*mut Object; 6] = [
                msg_send![class!(NSNumber), numberWithUnsignedInt: LINEAR_PCM],
                msg_send![class!(NSNumber), numberWithDouble: 16_000.0f64],
                msg_send![class!(NSNumber), numberWithInt: 1i32],
                msg_send![class!(NSNumber), numberWithInt: 16i32],
                msg_send![class!(NSNumber), numberWithBool: NO],
                msg_send![class!(NSNumber), numberWithBool: NO],
            ];
            let settings: *mut Object =
                msg_send![class!(NSDictionary), dictionaryWithObjects: values.as_ptr() forKeys: keys.as_ptr() count: keys.len()];
            let mut error: *mut Object = std::ptr::null_mut();
            let recorder: *mut Object = msg_send![class!(AVAudioRecorder), alloc];
            // The `.wav` extension picks the WAVE container.
            let recorder: *mut Object = msg_send![recorder, initWithURL: url settings: settings error: &mut error];
            if recorder.is_null() {
                bail!("{}", describe(error));
            }
            let _: () = msg_send![recorder, setMeteringEnabled: YES];
            let started: BOOL = msg_send![recorder, record];
            if started == NO {
                let _: () = msg_send![recorder, release];
                bail!("the microphone could not be started");
            }
            Ok(Self { recorder })
        }

        pub fn level(&self) -> f32 {
            unsafe {
                let _: () = msg_send![self.recorder, updateMeters];
                let db: f32 = msg_send![self.recorder, averagePowerForChannel: 0usize];
                // -50 dB and below reads as silence; 0 dB is full scale.
                ((db + 50.0) / 50.0).clamp(0.0, 1.0)
            }
        }
    }

    impl Drop for Recorder {
        fn drop(&mut self) {
            unsafe {
                // `stop` closes the file before it returns.
                let _: () = msg_send![self.recorder, stop];
                let _: () = msg_send![self.recorder, release];
            }
        }
    }

    unsafe fn describe(error: *mut Object) -> String {
        if error.is_null() {
            return "the microphone could not be opened".into();
        }
        let text: *mut Object = msg_send![error, localizedDescription];
        let utf8: *const std::os::raw::c_char = if text.is_null() { std::ptr::null() } else { msg_send![text, UTF8String] };
        if utf8.is_null() {
            "the microphone could not be opened".into()
        } else {
            CStr::from_ptr(utf8).to_string_lossy().into_owned()
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::Permission;
    use std::path::Path;

    pub fn permission() -> Permission {
        Permission::Denied
    }

    pub fn request_permission(done: impl FnOnce(bool) + Send + 'static) {
        done(false)
    }

    pub fn open_privacy_settings() {}

    pub struct Recorder;

    impl Recorder {
        pub fn start(_to: &Path) -> anyhow::Result<Self> {
            anyhow::bail!("voice input is not available on this platform yet")
        }

        pub fn level(&self) -> f32 {
            0.0
        }
    }
}
