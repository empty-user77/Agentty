//! Windows: embeds Agentty's icon (resource 1, which GPUI uses for the window and taskbar icon;
//! Explorer shows it for the .exe) and version information. Other platforms need nothing here.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
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
