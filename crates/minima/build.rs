//! Bakes the content model (weights + lexicon) into the binary.
//!
//! `model/weights.txt` and `model/lexicon.txt` are parsed here and emitted as
//! static Rust tables in `$OUT_DIR/model_data.rs`, so nothing is loaded or
//! parsed from disk when Minima runs.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=model/weights.txt");
    println!("cargo:rerun-if-changed=model/lexicon.txt");
    println!("cargo:rerun-if-changed=build.rs");

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let weights = fs::read_to_string(manifest.join("model/weights.txt")).expect("weights.txt");
    let lexicon = fs::read_to_string(manifest.join("model/lexicon.txt")).expect("lexicon.txt");

    let mut out = String::new();

    // ---- weights -------------------------------------------------------
    out.push_str("pub static WEIGHTS: &[(&str, f32)] = &[\n");
    for line in weights.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let name = it.next().unwrap();
        let val: f32 = it
            .next()
            .unwrap_or_else(|| panic!("weight without value: {line}"))
            .parse()
            .unwrap_or_else(|_| panic!("bad weight: {line}"));
        writeln!(out, "    ({:?}, {:?}),", name, val).unwrap();
    }
    out.push_str("];\n\n");

    // ---- lexicon -------------------------------------------------------
    let mut sections: Vec<(String, Vec<String>)> = Vec::new();
    for line in lexicon.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            sections.push((line[1..line.len() - 1].to_string(), Vec::new()));
        } else if let Some(s) = sections.last_mut() {
            s.1.push(line.to_string());
        }
    }
    let flat = |name: &str| -> Vec<String> {
        sections
            .iter()
            .filter(|s| s.0 == name)
            .flat_map(|s| s.1.iter())
            .flat_map(|l| l.split(',').map(|w| w.trim().to_lowercase()))
            .filter(|w| !w.is_empty())
            .collect()
    };
    for (cname, sname) in [
        ("STOPWORDS", "stopwords"),
        ("CLASS_POS", "class_pos"),
        ("CLASS_NEG", "class_neg"),
        ("FORM_POS", "form_pos"),
        ("FORM_NEG", "form_neg"),
    ] {
        writeln!(out, "pub static {cname}: &[&str] = &[").unwrap();
        for w in flat(sname) {
            writeln!(out, "    {:?},", w).unwrap();
        }
        out.push_str("];\n\n");
    }
    out.push_str("pub static SYNONYMS: &[&[&str]] = &[\n");
    for s in sections.iter().filter(|s| s.0 == "synonyms") {
        for line in &s.1 {
            let words: Vec<String> = line
                .split(',')
                .map(|w| w.trim().to_lowercase())
                .filter(|w| !w.is_empty())
                .collect();
            writeln!(out, "    &{:?},", words).unwrap();
        }
    }
    out.push_str("];\n");

    embed_windows_resources("Minima terminal web browser", "minima.exe");
    let dest = PathBuf::from(env::var("OUT_DIR").unwrap()).join("model_data.rs");
    fs::write(dest, out).unwrap();
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
