//! Credential storage provided by the operating system.
//!
//! - macOS: the login Keychain (Security.framework), exactly as before.
//! - Windows: Credential Manager (generic credentials, per-user, encrypted with DPAPI).
//! - Linux: the Secret Service (GNOME Keyring / KWallet) through `secret-tool`; when no Secret
//!   Service is running (headless machines, minimal desktops) a private file
//!   (`<data dir>/secrets/<service>.json`, `0600` in a `0700` directory) is used instead.
//!
//! Secrets are addressed by `(service, account)`; nothing here ever prints or logs a value.

use anyhow::Result;

/// Human-readable name of the store that holds secrets on this machine (shown in Settings).
pub fn backend_name() -> &'static str {
    imp::backend_name()
}

pub fn store(service: &str, account: &str, secret: &str) -> Result<()> {
    imp::store(service, account, secret)
}

pub fn load(service: &str, account: &str) -> Result<String> {
    imp::load(service, account)
}

/// Removes a secret; removing one that does not exist is not an error.
pub fn delete(service: &str, account: &str) -> Result<()> {
    imp::delete(service, account)
}

#[cfg(target_os = "macos")]
mod imp {
    use anyhow::Result;

    pub fn backend_name() -> &'static str {
        "Keychain"
    }

    pub fn store(service: &str, account: &str, secret: &str) -> Result<()> {
        security_framework::passwords::set_generic_password(service, account, secret.as_bytes())?;
        Ok(())
    }

    pub fn load(service: &str, account: &str) -> Result<String> {
        let bytes = security_framework::passwords::get_generic_password(service, account)?;
        Ok(String::from_utf8(bytes)?)
    }

    pub fn delete(service: &str, account: &str) -> Result<()> {
        security_framework::passwords::delete_generic_password(service, account)?;
        Ok(())
    }
}

#[cfg(windows)]
mod imp {
    use anyhow::{bail, Result};
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND, FILETIME};
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    pub fn backend_name() -> &'static str {
        "Windows Credential Manager"
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn target(service: &str, account: &str) -> Vec<u16> {
        wide(&format!("{service}:{account}"))
    }

    pub fn store(service: &str, account: &str, secret: &str) -> Result<()> {
        let mut target = target(service, account);
        let mut user = wide(account);
        let mut blob = secret.as_bytes().to_vec();
        let credential = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            Comment: std::ptr::null_mut(),
            LastWritten: FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 },
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: std::ptr::null_mut(),
            TargetAlias: std::ptr::null_mut(),
            UserName: user.as_mut_ptr(),
        };
        // SAFETY: every pointer in `credential` refers to a live buffer owned by this function.
        let ok = unsafe { CredWriteW(&credential, 0) };
        blob.iter_mut().for_each(|b| *b = 0);
        if ok == 0 {
            bail!("Credential Manager refused the secret (error {})", unsafe { GetLastError() });
        }
        Ok(())
    }

    pub fn load(service: &str, account: &str) -> Result<String> {
        let target = target(service, account);
        let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `target` is NUL-terminated; on success `credential` is freed with CredFree below.
        let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };
        if ok == 0 || credential.is_null() {
            bail!("secret not found in Credential Manager");
        }
        // SAFETY: CredReadW returned a valid credential whose blob has `CredentialBlobSize` bytes.
        let bytes = unsafe {
            let c = &*credential;
            let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec();
            CredFree(credential.cast());
            bytes
        };
        Ok(String::from_utf8(bytes)?)
    }

    pub fn delete(service: &str, account: &str) -> Result<()> {
        let target = target(service, account);
        // SAFETY: `target` is NUL-terminated.
        let ok = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if ok == 0 && unsafe { GetLastError() } != ERROR_NOT_FOUND {
            bail!("could not remove the secret from Credential Manager");
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use anyhow::{bail, Context, Result};
    use std::io::Write;
    use std::process::{Command, Stdio};

    pub fn backend_name() -> &'static str {
        if secret_service_available() {
            "Secret Service"
        } else {
            "~/.agentty/secrets (0600)"
        }
    }

    /// `secret-tool` is installed and a Secret Service answers on the session bus.
    fn secret_service_available() -> bool {
        static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *AVAILABLE.get_or_init(|| {
            if std::env::var_os("AGENTTY_SECRET_FILE").is_some() || std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
                return false;
            }
            // A lookup of an attribute nobody uses: exit status 1 means "not found" from a working
            // service; anything else (missing binary, no service) means unavailable.
            Command::new("secret-tool")
                .args(["lookup", "agentty-probe", "1"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| matches!(s.code(), Some(0 | 1)))
        })
    }

    pub fn store(service: &str, account: &str, secret: &str) -> Result<()> {
        if secret_service_available() {
            let mut child = Command::new("secret-tool")
                .args(["store", "--label", &format!("Agentty ({service})"), "service", service, "account", account])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("secret-tool could not be started")?;
            // The secret goes over stdin, never on the command line.
            child.stdin.take().context("secret-tool stdin")?.write_all(secret.as_bytes())?;
            if child.wait()?.success() {
                return Ok(());
            }
        }
        file::store(service, account, secret)
    }

    pub fn load(service: &str, account: &str) -> Result<String> {
        if secret_service_available() {
            let output = Command::new("secret-tool")
                .args(["lookup", "service", service, "account", account])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()?;
            if output.status.success() && !output.stdout.is_empty() {
                // Some versions end the secret with a newline when printing it.
                let secret = String::from_utf8(output.stdout)?;
                return Ok(secret.strip_suffix('\n').unwrap_or(&secret).to_string());
            }
        }
        match file::load(service, account) {
            Some(secret) => Ok(secret),
            None => bail!("secret not found"),
        }
    }

    pub fn delete(service: &str, account: &str) -> Result<()> {
        if secret_service_available() {
            let _ = Command::new("secret-tool")
                .args(["clear", "service", service, "account", account])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        file::delete(service, account)
    }

    /// Fallback: one JSON object per service in a private directory.
    pub(super) mod file {
        use anyhow::Result;
        use std::collections::BTreeMap;
        use std::path::{Path, PathBuf};

        fn path(base: &Path, service: &str) -> PathBuf {
            let name: String = service.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '_' }).collect();
            base.join("secrets").join(format!("{name}.json"))
        }

        fn read(base: &Path, service: &str) -> BTreeMap<String, String> {
            std::fs::read(path(base, service)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
        }

        fn write(base: &Path, service: &str, entries: &BTreeMap<String, String>) -> Result<()> {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let path = path(base, service);
            let dir = path.parent().expect("secrets file has a directory");
            std::fs::create_dir_all(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            let tmp = path.with_extension("json.tmp");
            let _ = std::fs::remove_file(&tmp);
            // Created 0600 from the start, so the secret is never readable by others, even briefly.
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
            std::io::Write::write_all(&mut file, &serde_json::to_vec(entries)?)?;
            drop(file);
            std::fs::rename(tmp, path)?;
            Ok(())
        }

        pub fn store_at(base: &Path, service: &str, account: &str, secret: &str) -> Result<()> {
            let mut entries = read(base, service);
            entries.insert(account.to_string(), secret.to_string());
            write(base, service, &entries)
        }

        pub fn load_at(base: &Path, service: &str, account: &str) -> Option<String> {
            read(base, service).remove(account)
        }

        pub fn delete_at(base: &Path, service: &str, account: &str) -> Result<()> {
            let mut entries = read(base, service);
            if entries.remove(account).is_some() {
                write(base, service, &entries)?;
            }
            Ok(())
        }

        pub fn store(service: &str, account: &str, secret: &str) -> Result<()> {
            store_at(&crate::fsutil::data_dir(), service, account, secret)
        }

        pub fn load(service: &str, account: &str) -> Option<String> {
            load_at(&crate::fsutil::data_dir(), service, account)
        }

        pub fn delete(service: &str, account: &str) -> Result<()> {
            delete_at(&crate::fsutil::data_dir(), service, account)
        }
    }
}

#[cfg(all(test, not(any(target_os = "macos", windows))))]
mod tests {
    #[test]
    fn file_fallback_roundtrip_is_private() {
        use super::imp::file;
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("agentty-secrets-{}", std::process::id()));
        let value = format!("{}_{}", "example", "not_a_real_key");
        file::store_at(&dir, "run.agentty.test", "one", &value).unwrap();
        assert_eq!(file::load_at(&dir, "run.agentty.test", "one").as_deref(), Some(value.as_str()));
        let path = dir.join("secrets").join("run.agentty.test.json");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        file::delete_at(&dir, "run.agentty.test", "one").unwrap();
        assert!(file::load_at(&dir, "run.agentty.test", "one").is_none());
        std::fs::remove_dir_all(dir).ok();
    }
}
