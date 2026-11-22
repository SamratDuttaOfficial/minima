//! Packaging tasks, written in Rust so they behave identically on Windows,
//! Linux and macOS (no tar/ar/zip/sha256sum dependency).
//!
//!   xtask dist --bin PATH --os OS --arch ARCH --version V --out DIR [--installer PATH]
//!   xtask sums DIR
//!
//! `dist` produces, inside DIR:
//!   minima-V-OS-ARCH.zip | .tar.gz      portable archive
//!   minima_V_ARCH.deb                   (linux)  proper .deb package
//!   minima-V-OS-ARCH-installer-cli[.exe] minimal self-contained CLI installer

mod icons;

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use flate2::{write::GzEncoder, Compression};
use sha2::{Digest, Sha256};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(|s| s.as_str()) {
        Some("dist") => dist(&args[1..]),
        Some("sums") => sums(args.get(1).map(|s| s.as_str()).unwrap_or("dist")),
        Some("icons") => icons::generate(&root().join("assets")),
        _ => Err("usage: xtask dist --bin PATH --os OS --arch ARCH --version V --out DIR [--installer PATH] | xtask sums DIR".into()),
    };
    if let Err(e) = r {
        eprintln!("xtask: {e}");
        std::process::exit(1);
    }
}

fn opt(args: &[String], key: &str) -> Option<String> {
    args.iter().position(|a| a == key).and_then(|i| args.get(i + 1)).cloned()
}

fn root() -> PathBuf {
    // crates/xtask → workspace root
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap_or_else(|_| PathBuf::from("."))
}

fn dist(args: &[String]) -> Result<(), String> {
    let bin = PathBuf::from(opt(args, "--bin").ok_or("--bin required")?);
    let os = opt(args, "--os").ok_or("--os required")?;
    let arch = opt(args, "--arch").ok_or("--arch required")?;
    let version = opt(args, "--version").unwrap_or_else(|| "0.0.0".into());
    let out = PathBuf::from(opt(args, "--out").unwrap_or_else(|| "dist".into()));
    let installer = opt(args, "--installer").map(PathBuf::from);
    fs::create_dir_all(&out).map_err(|e| e.to_string())?;

    let exe_name = if os == "windows" { "minima.exe" } else { "minima" };
    let bin_bytes = fs::read(&bin).map_err(|e| format!("{}: {e}", bin.display()))?;
    let readme = fs::read(root().join("README.md")).unwrap_or_default();
    let license = fs::read(root().join("LICENSE")).unwrap_or_default();
    let base = format!("minima-{version}-{os}-{arch}");

    // ---- portable archive ---------------------------------------------
    if os == "windows" {
        let path = out.join(format!("{base}.zip"));
        let f = File::create(&path).map_err(|e| e.to_string())?;
        let mut z = zip::ZipWriter::new(f);
        let exec = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).unix_permissions(0o755);
        let plain = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).unix_permissions(0o644);
        let dir = format!("{base}/");
        z.start_file(format!("{dir}{exe_name}"), exec).map_err(|e| e.to_string())?;
        z.write_all(&bin_bytes).map_err(|e| e.to_string())?;
        z.start_file(format!("{dir}README.md"), plain).map_err(|e| e.to_string())?;
        z.write_all(&readme).map_err(|e| e.to_string())?;
        if !license.is_empty() {
            z.start_file(format!("{dir}LICENSE"), plain).map_err(|e| e.to_string())?;
            z.write_all(&license).map_err(|e| e.to_string())?;
        }
        z.finish().map_err(|e| e.to_string())?;
        println!("  wrote {}", path.display());
    } else {
        let path = out.join(format!("{base}.tar.gz"));
        let gz = GzEncoder::new(File::create(&path).map_err(|e| e.to_string())?, Compression::best());
        let mut t = tar::Builder::new(gz);
        append(&mut t, &format!("{base}/{exe_name}"), &bin_bytes, 0o755)?;
        append(&mut t, &format!("{base}/README.md"), &readme, 0o644)?;
        if !license.is_empty() {
            append(&mut t, &format!("{base}/LICENSE"), &license, 0o644)?;
        }
        t.into_inner().map_err(|e| e.to_string())?.finish().map_err(|e| e.to_string())?;
        println!("  wrote {}", path.display());
    }

    // ---- .deb ------------------------------------------------------------
    if os == "linux" {
        let deb_arch = match arch.as_str() {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            "armv7" => "armhf",
            "i686" => "i386",
            other => other,
        };
        let path = out.join(format!("minima_{version}_{deb_arch}.deb"));
        build_deb(&path, &version, deb_arch, &bin_bytes, &readme)?;
        println!("  wrote {}", path.display());
    }

    // ---- minimal CLI installer ---------------------------------------
    if let Some(inst) = installer {
        let ext = if os == "windows" { ".exe" } else { "" };
        let dest = out.join(format!("{base}-installer-cli{ext}"));
        fs::copy(&inst, &dest).map_err(|e| format!("{}: {e}", inst.display()))?;
        println!("  wrote {}", dest.display());
    }
    Ok(())
}

fn append<W: Write>(t: &mut tar::Builder<W>, name: &str, data: &[u8], mode: u32) -> Result<(), String> {
    let mut h = tar::Header::new_gnu();
    h.set_size(data.len() as u64);
    h.set_mode(mode);
    h.set_mtime(now());
    h.set_uid(0);
    h.set_gid(0);
    h.set_cksum();
    t.append_data(&mut h, name, data).map_err(|e| e.to_string())
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn tar_gz(entries: &[(String, Vec<u8>, u32)], dirs: &[&str]) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    {
        let gz = GzEncoder::new(&mut buf, Compression::best());
        let mut t = tar::Builder::new(gz);
        for d in dirs {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Directory);
            h.set_size(0);
            h.set_mode(0o755);
            h.set_mtime(now());
            h.set_uid(0);
            h.set_gid(0);
            h.set_cksum();
            t.append_data(&mut h, d, &[][..]).map_err(|e| e.to_string())?;
        }
        for (name, data, mode) in entries {
            append(&mut t, name, data, *mode)?;
        }
        t.into_inner().map_err(|e| e.to_string())?.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf)
}

fn build_deb(path: &Path, version: &str, arch: &str, bin: &[u8], readme: &[u8]) -> Result<(), String> {
    let desktop = "[Desktop Entry]\nType=Application\nName=Minima\nGenericName=Terminal Web Browser\nComment=Browse the web in your terminal\nExec=minima\nTerminal=true\nCategories=Network;WebBrowser;ConsoleOnly;\nKeywords=browser;terminal;text;\n";
    let installed_kb = (bin.len() + readme.len() + desktop.len()) / 1024 + 1;
    let control = format!(
        "Package: minima\nVersion: {version}\nSection: web\nPriority: optional\nArchitecture: {arch}\nInstalled-Size: {installed_kb}\nMaintainer: Samrat Dutta <samratduttaofficial@gmail.com>\nRecommends: ffmpeg\nSuggests: yt-dlp\nDescription: terminal web browser that strips the web down to text\n Minima fetches web pages, removes ads, menus and banners with a compiled-in\n content model, and shows links, buttons and forms in the terminal. Images\n and video are rendered as ASCII / braille art.\n"
    );
    let control_tar = tar_gz(&[("control".into(), control.into_bytes(), 0o644)], &["./"])?;
    let data_tar = tar_gz(
        &[
            ("usr/bin/minima".into(), bin.to_vec(), 0o755),
            ("usr/share/applications/minima.desktop".into(), desktop.as_bytes().to_vec(), 0o644),
            ("usr/share/doc/minima/README.md".into(), readme.to_vec(), 0o644),
            ("usr/share/icons/hicolor/256x256/apps/minima.png".into(), fs::read(root().join("assets/minima-256.png")).unwrap_or_default(), 0o644),
            ("usr/share/icons/hicolor/48x48/apps/minima.png".into(), fs::read(root().join("assets/minima-48.png")).unwrap_or_default(), 0o644),
            ("usr/share/icons/hicolor/scalable/apps/minima.svg".into(), fs::read(root().join("assets/minima.svg")).unwrap_or_default(), 0o644),
        ],
        &["./", "usr/", "usr/bin/", "usr/share/", "usr/share/applications/", "usr/share/doc/", "usr/share/doc/minima/", "usr/share/icons/", "usr/share/icons/hicolor/", "usr/share/icons/hicolor/256x256/", "usr/share/icons/hicolor/256x256/apps/", "usr/share/icons/hicolor/48x48/", "usr/share/icons/hicolor/48x48/apps/", "usr/share/icons/hicolor/scalable/", "usr/share/icons/hicolor/scalable/apps/"],
    )?;
    let mut f = File::create(path).map_err(|e| e.to_string())?;
    f.write_all(b"!<arch>\n").map_err(|e| e.to_string())?;
    ar_member(&mut f, "debian-binary", b"2.0\n")?;
    ar_member(&mut f, "control.tar.gz", &control_tar)?;
    ar_member(&mut f, "data.tar.gz", &data_tar)?;
    Ok(())
}

fn ar_member<W: Write>(w: &mut W, name: &str, data: &[u8]) -> Result<(), String> {
    let header = format!("{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n", name, now(), 0, 0, "100644", data.len());
    debug_assert_eq!(header.len(), 60);
    w.write_all(header.as_bytes()).map_err(|e| e.to_string())?;
    w.write_all(data).map_err(|e| e.to_string())?;
    if data.len() % 2 == 1 {
        w.write_all(b"\n").map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn sums(dir: &str) -> Result<(), String> {
    let dir = PathBuf::from(dir);
    let mut lines = Vec::new();
    let mut names: Vec<_> = fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n != "SHA256SUMS")
        .collect();
    names.sort();
    for n in names {
        let mut f = File::open(dir.join(&n)).map_err(|e| e.to_string())?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let k = f.read(&mut buf).map_err(|e| e.to_string())?;
            if k == 0 {
                break;
            }
            h.update(&buf[..k]);
        }
        let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        lines.push(format!("{hex}  {n}"));
    }
    fs::write(dir.join("SHA256SUMS"), lines.join("\n") + "\n").map_err(|e| e.to_string())?;
    println!("  wrote {}", dir.join("SHA256SUMS").display());
    Ok(())
}
