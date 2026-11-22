//! Embeds the compiled `minima` binary into the installer.
//!
//! Set `MINIMA_PAYLOAD=/path/to/minima[.exe]` when building. Without it the
//! installer is built empty (useful for `cargo check`) and refuses to install.

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=MINIMA_PAYLOAD");
    embed_windows_resources("Minima installer", "minima-installer.exe");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("payload.bin");
    match env::var("MINIMA_PAYLOAD") {
        Ok(p) if !p.is_empty() => {
            println!("cargo:rerun-if-changed={p}");
            fs::copy(&p, &out).unwrap_or_else(|e| panic!("cannot read MINIMA_PAYLOAD={p}: {e}"));
        }
        _ => {
            fs::write(&out, b"").unwrap();
            println!("cargo:warning=MINIMA_PAYLOAD not set: building an empty installer");
        }
    }
}

/// Windows: embed the app icon and version info into the executable.
fn embed_windows_resources(description: &str, original: &str) {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico = manifest.join("../../assets/minima.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    let ver = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let nums: Vec<String> = ver.split('.').map(|s| s.to_string()).chain(std::iter::repeat("0".to_string())).take(4).collect();
    let comma = nums.join(",");
    let rc = format!(
        "1 ICON \"{ico}\"\n\
         1 VERSIONINFO\n FILEVERSION {comma}\n PRODUCTVERSION {comma}\n FILEOS 0x40004\n FILETYPE 0x1\n\
         BEGIN\n BLOCK \"StringFileInfo\"\n BEGIN\n  BLOCK \"040904b0\"\n  BEGIN\n\
           VALUE \"CompanyName\", \"Minima\"\n\
           VALUE \"FileDescription\", \"{description}\"\n\
           VALUE \"FileVersion\", \"{ver}\"\n\
           VALUE \"InternalName\", \"{original}\"\n\
           VALUE \"OriginalFilename\", \"{original}\"\n\
           VALUE \"ProductName\", \"Minima\"\n\
           VALUE \"ProductVersion\", \"{ver}\"\n\
           VALUE \"LegalCopyright\", \"MIT License\"\n\
          END\n END\n BLOCK \"VarFileInfo\"\n BEGIN\n  VALUE \"Translation\", 0x409, 1200\n END\n\
         END\n",
        ico = ico.canonicalize().unwrap_or(ico.clone()).display().to_string().trim_start_matches("\\\\?\\").replace('\\', "/"),
    );
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("minima.rc");
    fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE).manifest_optional().unwrap();
}
