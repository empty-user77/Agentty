//! Every platform: the build information Settings → About shows (`AGENTTY_BUILD_*`).
//! Windows: also embeds Agentty's icon (resource 1, which GPUI uses for the window and taskbar
//! icon; Explorer shows it for the .exe) and version information.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    build_info::emit();
    // Build scripts run on the host; resources are only compiled when building on Windows
    // (cross-checks from other hosts skip them, as GPUI's own manifest does).
    #[cfg(windows)]
    windows::embed();
}

#[cfg(windows)]
mod windows {
    use std::path::PathBuf;

    pub fn embed() {
        let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
        let icon = manifest_dir.join("../../packaging/windows/agentty.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
        let numbers: Vec<u16> = version.split(['.', '-', '+']).take(3).map(|part| part.parse().unwrap_or(0)).collect();
        let [major, minor, patch] = [0, 1, 2].map(|i| numbers.get(i).copied().unwrap_or(0));
        let icon_path = icon.display().to_string().replace('\\', "\\\\");
        let rc = format!(
            r#"1 ICON "{icon_path}"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Agentty contributors"
      VALUE "FileDescription", "Agentty"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "agentty"
      VALUE "LegalCopyright", "Apache-2.0"
      VALUE "OriginalFilename", "agentty.exe"
      VALUE "ProductName", "Agentty"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
        );
        let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("agentty.rc");
        std::fs::write(&out, rc).expect("write agentty.rc");
        embed_resource::compile(&out, embed_resource::NONE).manifest_optional().expect("compile Windows resources");
    }
}

/// Commit, date, profile, target and compiler of this build. Nothing here fails the build: what
/// cannot be found (no git in a source tarball, say) is "unknown". Only names and numbers leave
/// this script, never a path, so no folder of the machine that built it ends up in the binary.
mod build_info {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    pub fn emit() {
        let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
        let commit = git(&manifest_dir, &["rev-parse", "--short=9", "HEAD"]).unwrap_or_else(|| "unknown".into());
        watch_head(&manifest_dir);
        println!("cargo:rustc-env=AGENTTY_BUILD_COMMIT={commit}");
        println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
        println!("cargo:rustc-env=AGENTTY_BUILD_DATE={}", build_date());
        let env = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "unknown".into());
        println!("cargo:rustc-env=AGENTTY_BUILD_PROFILE={}", env("PROFILE"));
        println!("cargo:rustc-env=AGENTTY_BUILD_TARGET={}", env("TARGET"));
        println!("cargo:rustc-env=AGENTTY_BUILD_RUSTC={}", rustc_version().unwrap_or_else(|| "unknown".into()));
    }

    fn git(dir: &Path, args: &[&str]) -> Option<String> {
        let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
        let text = String::from_utf8(out.stdout).ok()?.trim().to_string();
        (out.status.success() && !text.is_empty()).then_some(text)
    }

    /// Runs again when HEAD moves (a checkout, a commit) — `.git/HEAD` and the branch it names,
    /// not the working tree, so editing a file does not rerun this script. Only files that exist
    /// are named: Cargo reruns on every build for one that does not.
    fn watch_head(dir: &Path) {
        let path = |args: &[&str]| git(dir, args).map(|p| dir.join(p));
        let (Some(git_dir), Some(common_dir)) = (path(&["rev-parse", "--git-dir"]), path(&["rev-parse", "--git-common-dir"])) else {
            return;
        };
        let mut watched = vec![git_dir.join("HEAD")];
        if let Some(branch) = git(dir, &["symbolic-ref", "-q", "HEAD"]) {
            // A worktree keeps its own HEAD; the branches live in the shared folder, loose or packed.
            let loose = common_dir.join(&branch);
            watched.push(if loose.is_file() { loose } else { common_dir.join("packed-refs") });
        }
        for file in watched.into_iter().filter(|f| f.is_file()) {
            println!("cargo:rerun-if-changed={}", file.display());
        }
    }

    /// `YYYY-MM-DD` in UTC: `SOURCE_DATE_EPOCH` when set (reproducible builds), otherwise today.
    fn build_date() -> String {
        let secs = std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|v| v.trim().parse::<i64>().ok())
            .or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64));
        let Some(secs) = secs else { return "unknown".into() };
        // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
        let z = secs.div_euclid(86_400) + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        format!("{year:04}-{month:02}-{day:02}")
    }

    /// `1.98.0` from `rustc 1.98.0 (abcdef 2026-01-01)`: the compiler that actually built this.
    fn rustc_version() -> Option<String> {
        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
        let out = Command::new(rustc).arg("--version").output().ok()?;
        let text = String::from_utf8(out.stdout).ok()?;
        let version = text.split_whitespace().nth(1)?;
        version.chars().all(|c| c.is_ascii_alphanumeric() || ".-".contains(c)).then(|| version.to_string())
    }
}
