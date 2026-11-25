//! Embeds the icon, version information and an application manifest into rustle.exe (Windows).

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/rustle.manifest");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        use std::path::PathBuf;

        let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let fwd = |p: PathBuf| p.to_string_lossy().replace('\\', "/");

        let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
        let mut nums: Vec<u32> = version
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|p| p.parse().ok())
            .collect();
        nums.resize(4, 0);
        let (a, b, c, d) = (nums[0], nums[1], nums[2], nums[3]);

        let rc = format!(
            r#"1 ICON "{icon}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {a},{b},{c},{d}
PRODUCTVERSION {a},{b},{c},{d}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "ProductName", "Rustle"
      VALUE "FileDescription", "Rustle - a native Git client"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "InternalName", "rustle"
      VALUE "OriginalFilename", "rustle.exe"
      VALUE "LegalCopyright", "MIT"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0409, 1200
  END
END
"#,
            icon = fwd(manifest_dir.join("assets/icon.ico")),
            manifest = fwd(manifest_dir.join("assets/rustle.manifest")),
        );
        let rc_path = out_dir.join("rustle.rc");
        std::fs::write(&rc_path, rc).expect("write rc file");
        embed_resource::compile(rc_path, embed_resource::NONE)
            .manifest_required()
            .expect("compile Windows resources");
    }
}
