//! Minima's minimal installer: one self-contained executable, no GUI, no
//! dependencies. Works over SSH, on servers and old machines.
//!
//!   minima-installer [--system | --user] [--prefix DIR] [--no-path]
//!                    [--with-media] [-y] [--dry-run]
//!   minima-installer --uninstall [--system | --user] [--prefix DIR]

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

static PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.bin"));
#[cfg(not(windows))]
static ICON_256: &[u8] = include_bytes!("../../../assets/minima-256.png");
#[cfg(not(windows))]
static ICON_48: &[u8] = include_bytes!("../../../assets/minima-48.png");
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(windows)]
const EXE: &str = "minima.exe";
#[cfg(not(windows))]
const EXE: &str = "minima";

struct Opts {
    system: bool,
    prefix: Option<PathBuf>,
    modify_path: bool,
    with_media: bool,
    yes: bool,
    dry: bool,
    uninstall: bool,
}

fn usage() {
    println!(
        "Minima {VERSION} installer (minimal, no GUI)\n\n\
         USAGE\n  minima-installer [OPTIONS]\n\n\
         OPTIONS\n\
           --user          install for the current user (default)\n\
           --system        install for all users (needs admin / root)\n\
           --prefix DIR    install into DIR (binary goes to DIR/bin on Unix, DIR on Windows)\n\
           --no-path       do not touch PATH\n\
           --with-media    also install ffmpeg + yt-dlp (video with sound, YouTube)\n\
           --uninstall     remove Minima\n\
           -y, --yes       do not ask questions\n\
           --dry-run       show what would happen\n\
           -h, --help      this text"
    );
}

fn parse() -> Opts {
    let mut o = Opts { system: false, prefix: None, modify_path: true, with_media: false, yes: false, dry: false, uninstall: false };
    let mut it = env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--system" => o.system = true,
            "--user" => o.system = false,
            "--prefix" => o.prefix = it.next().map(PathBuf::from),
            "--no-path" => o.modify_path = false,
            "--with-media" => o.with_media = true,
            "--uninstall" => o.uninstall = true,
            "-y" | "--yes" => o.yes = true,
            "--dry-run" => o.dry = true,
            "-h" | "--help" => {
                usage();
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown option: {other}\n");
                usage();
                std::process::exit(2);
            }
        }
    }
    o
}

fn home() -> PathBuf {
    env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// Directory that will contain the `minima` executable.
fn bin_dir(o: &Opts) -> PathBuf {
    if let Some(p) = &o.prefix {
        return if cfg!(windows) { p.clone() } else { p.join("bin") };
    }
    if cfg!(windows) {
        if o.system {
            PathBuf::from(env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into())).join("Minima")
        } else {
            PathBuf::from(env::var("LOCALAPPDATA").unwrap_or_else(|_| home().join("AppData\\Local").to_string_lossy().into())).join("Programs").join("Minima")
        }
    } else if o.system {
        PathBuf::from("/usr/local/bin")
    } else {
        home().join(".local").join("bin")
    }
}

fn ask(o: &Opts, q: &str) -> bool {
    if o.yes {
        return true;
    }
    print!("{q} [Y/n] ");
    let _ = io::stdout().flush();
    let mut s = String::new();
    if io::stdin().read_line(&mut s).is_err() || s.is_empty() {
        return true; // non-interactive: accept defaults
    }
    !matches!(s.trim().to_lowercase().as_str(), "n" | "no")
}

fn run(o: &Opts, prog: &str, args: &[&str]) -> bool {
    println!("  $ {} {}", prog, args.join(" "));
    if o.dry {
        return true;
    }
    Command::new(prog).args(args).status().map(|s| s.success()).unwrap_or(false)
}

fn main() {
    let o = parse();
    if o.uninstall {
        return uninstall(&o);
    }
    if PAYLOAD.is_empty() {
        eprintln!("This installer was built without the Minima binary (MINIMA_PAYLOAD was not set).");
        std::process::exit(1);
    }
    let dir = bin_dir(&o);
    let target = dir.join(EXE);
    println!("Minima {VERSION}");
    println!("  install to: {}", target.display());
    if !ask(&o, "Proceed?") {
        println!("cancelled");
        return;
    }
    if o.dry {
        println!("  (dry run: {} bytes would be written)", PAYLOAD.len());
    } else {
        if let Err(e) = fs::create_dir_all(&dir) {
            fail(&format!("cannot create {}: {e}{}", dir.display(), hint_perm(&o)));
        }
        // write via temp + rename so an upgrade over a running copy works on Unix
        let tmp = dir.join(format!(".{EXE}.new"));
        if let Err(e) = fs::write(&tmp, PAYLOAD) {
            fail(&format!("cannot write {}: {e}{}", tmp.display(), hint_perm(&o)));
        }
        set_exec(&tmp);
        let old = dir.join(format!("{EXE}.old"));
        let _ = fs::remove_file(&old);
        if target.exists() && fs::remove_file(&target).is_err() {
            // a running program can't be deleted on Windows, but it can be renamed out of the way
            if fs::rename(&target, &old).is_ok() {
                println!("  (the old copy is in use; it was moved aside and will keep running until you close it)");
            }
        }
        if let Err(e) = fs::rename(&tmp, &target) {
            fail(&format!("cannot place {}: {e} (is Minima running?)", target.display()));
        }
        // keep a copy of ourselves for uninstalling on Windows
        if cfg!(windows) {
            if let Ok(me) = env::current_exe() {
                let _ = fs::copy(me, dir.join("minima-uninstall.exe"));
            }
        }
        println!("  installed {} ({} KB)", target.display(), PAYLOAD.len() / 1024);
    }

    platform_integration(&o, &dir, &target);

    if o.with_media || (!o.yes && ask_media(&o)) {
        install_media(&o, &dir);
    }
    println!("\nDone. Run: minima");
    if !path_contains(&dir) && !cfg!(windows) {
        println!("Note: {} is not on your PATH yet. Add this to your shell profile:\n  export PATH=\"{}:$PATH\"", dir.display(), dir.display());
    } else if cfg!(windows) {
        println!("Open a new terminal window for the PATH change to take effect.");
    }
}

fn ask_media(o: &Opts) -> bool {
    println!("\nOptional: ffmpeg + yt-dlp enable video with sound and YouTube (images, pages and GIFs work without them).");
    ask_default_no(o, "Install them now?")
}

fn ask_default_no(o: &Opts, q: &str) -> bool {
    if o.yes {
        return false;
    }
    print!("{q} [y/N] ");
    let _ = io::stdout().flush();
    let mut s = String::new();
    if io::stdin().read_line(&mut s).is_err() {
        return false;
    }
    matches!(s.trim().to_lowercase().as_str(), "y" | "yes")
}

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}

fn hint_perm(o: &Opts) -> &'static str {
    if o.system {
        if cfg!(windows) {
            "\n  (system install needs an elevated prompt: run as Administrator)"
        } else {
            "\n  (system install needs root: try sudo)"
        }
    } else {
        ""
    }
}

fn path_contains(dir: &Path) -> bool {
    env::var_os("PATH").map_or(false, |p| env::split_paths(&p).any(|d| d == dir))
}

#[cfg(unix)]
fn set_exec(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o755));
}
#[cfg(not(unix))]
fn set_exec(_: &Path) {}

// ---------------------------------------------------------------------------
// platform integration
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn platform_integration(o: &Opts, dir: &Path, target: &Path) {
    let scope = if o.system { "Machine" } else { "User" };
    if o.modify_path {
        println!("  adding {} to the {scope} PATH", dir.display());
        let d = dir.to_string_lossy().replace('\'', "''");
        let script = format!(
            "$d='{d}'; $p=[Environment]::GetEnvironmentVariable('Path','{scope}'); \
             if(-not $p){{$p=''}}; \
             if(($p -split ';') -notcontains $d){{ [Environment]::SetEnvironmentVariable('Path', ($p.TrimEnd(';')+';'+$d).TrimStart(';'), '{scope}') }}"
        );
        run(o, "powershell", &["-NoProfile", "-NonInteractive", "-Command", &script]);
    }
    // Start Menu shortcut
    let menu = if o.system {
        PathBuf::from(env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into())).join("Microsoft\\Windows\\Start Menu\\Programs")
    } else {
        PathBuf::from(env::var("APPDATA").unwrap_or_default()).join("Microsoft\\Windows\\Start Menu\\Programs")
    };
    let lnk = menu.join("Minima.lnk");
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}'); $s.TargetPath='{}'; $s.WorkingDirectory=$env:USERPROFILE; $s.Description='Minima terminal web browser'; $s.Save()",
        lnk.to_string_lossy().replace('\'', "''"),
        target.to_string_lossy().replace('\'', "''")
    );
    println!("  creating Start Menu shortcut");
    run(o, "powershell", &["-NoProfile", "-NonInteractive", "-Command", &script]);
    // Add/Remove Programs entry
    let hive = if o.system { "HKLM" } else { "HKCU" };
    let key = format!("{hive}\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Minima");
    let un = format!("\"{}\" --uninstall{}", dir.join("minima-uninstall.exe").display(), if o.system { " --system" } else { "" });
    for (name, val) in [
        ("DisplayName", "Minima".to_string()),
        ("DisplayVersion", VERSION.to_string()),
        ("Publisher", "Minima".to_string()),
        ("InstallLocation", dir.display().to_string()),
        ("DisplayIcon", target.display().to_string()),
        ("UninstallString", un),
    ] {
        run(o, "reg", &["add", &key, "/v", name, "/t", "REG_SZ", "/d", &val, "/f"]);
    }
}

#[cfg(not(windows))]
fn platform_integration(o: &Opts, dir: &Path, target: &Path) {
    let _ = dir;
    if o.dry {
        return;
    }
    // application icons (hicolor theme)
    let share = if o.system { PathBuf::from("/usr/share") } else { home().join(".local/share") };
    for (size, data) in [("256x256", ICON_256), ("48x48", ICON_48)] {
        let d = share.join("icons/hicolor").join(size).join("apps");
        if fs::create_dir_all(&d).is_ok() && fs::write(d.join("minima.png"), data).is_ok() {
            println!("  installed {size} icon");
        }
    }
    // application-menu entry (opens in the user's terminal)
    let apps = if o.system { PathBuf::from("/usr/share/applications") } else { home().join(".local/share/applications") };
    if fs::create_dir_all(&apps).is_ok() {
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName=Minima\nGenericName=Terminal Web Browser\nComment=Browse the web in your terminal\nExec={}\nTerminal=true\nCategories=Network;WebBrowser;ConsoleOnly;\nKeywords=browser;terminal;text;\n",
            target.display()
        );
        if fs::write(apps.join("minima.desktop"), entry).is_ok() {
            println!("  added application-menu entry");
        }
    }
    if o.modify_path && !o.system && !path_contains(&dir.to_path_buf()) {
        let line = format!("\nexport PATH=\"{}:$PATH\"  # added by minima-installer\n", dir.display());
        for rc in [".profile", ".bashrc", ".zshrc"] {
            let p = home().join(rc);
            if p.exists() {
                let cur = fs::read_to_string(&p).unwrap_or_default();
                if !cur.contains("added by minima-installer") {
                    if let Ok(mut f) = fs::OpenOptions::new().append(true).open(&p) {
                        let _ = f.write_all(line.as_bytes());
                        println!("  added {} to PATH in ~/{}", dir.display(), rc);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// optional media tools
// ---------------------------------------------------------------------------

fn install_media(o: &Opts, dir: &Path) {
    println!("\nInstalling media helpers (ffmpeg, yt-dlp)…");
    let have = |p: &str| Command::new(p).arg("--version").output().map(|_| true).unwrap_or(false);
    if cfg!(windows) {
        if have("winget") {
            run(o, "winget", &["install", "--id", "Gyan.FFmpeg", "-e", "--silent", "--accept-package-agreements", "--accept-source-agreements"]);
            run(o, "winget", &["install", "--id", "yt-dlp.yt-dlp", "-e", "--silent", "--accept-package-agreements", "--accept-source-agreements"]);
        } else {
            println!("  winget not found. Install ffmpeg (https://ffmpeg.org) and yt-dlp (https://github.com/yt-dlp/yt-dlp) manually.");
        }
    } else if cfg!(target_os = "macos") {
        if have("brew") {
            run(o, "brew", &["install", "ffmpeg", "yt-dlp"]);
        } else {
            println!("  Homebrew not found. Install it from https://brew.sh then run: brew install ffmpeg yt-dlp");
        }
    } else {
        let sudo = if is_root() { "" } else { "sudo" };
        let pm: Option<(&str, Vec<&str>)> = if have("apt-get") {
            Some(("apt-get", vec!["install", "-y", "ffmpeg"]))
        } else if have("dnf") {
            Some(("dnf", vec!["install", "-y", "ffmpeg-free"]))
        } else if have("pacman") {
            Some(("pacman", vec!["-S", "--noconfirm", "ffmpeg", "yt-dlp"]))
        } else if have("apk") {
            Some(("apk", vec!["add", "ffmpeg", "yt-dlp"]))
        } else if have("zypper") {
            Some(("zypper", vec!["install", "-y", "ffmpeg"]))
        } else {
            None
        };
        match pm {
            Some((p, args)) => {
                if sudo.is_empty() {
                    run(o, p, &args);
                } else {
                    let mut a = vec![p];
                    a.extend(args.iter().cloned());
                    run(o, sudo, &a);
                }
            }
            None => println!("  no known package manager; install ffmpeg manually"),
        }
        // yt-dlp ships as a single self-contained file
        if !have("yt-dlp") {
            let dest = dir.join("yt-dlp");
            println!("  fetching yt-dlp into {}", dest.display());
            let ok = run(o, "curl", &["-fsSL", "-o", &dest.to_string_lossy(), "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp"]);
            if ok && !o.dry {
                set_exec(&dest);
            }
        }
    }
}

#[cfg(unix)]
fn is_root() -> bool {
    Command::new("id").arg("-u").output().map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0").unwrap_or(false)
}
#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

// ---------------------------------------------------------------------------
// uninstall
// ---------------------------------------------------------------------------

fn uninstall(o: &Opts) {
    let dir = bin_dir(o);
    let target = dir.join(EXE);
    println!("Removing Minima from {}", dir.display());
    if !ask(o, "Proceed?") {
        return;
    }
    if !o.dry {
        let _ = fs::remove_file(&target);
        let _ = fs::remove_file(dir.join("yt-dlp"));
    }
    #[cfg(windows)]
    {
        let scope = if o.system { "Machine" } else { "User" };
        let d = dir.to_string_lossy().replace('\'', "''");
        let script = format!(
            "$d='{d}'; $p=[Environment]::GetEnvironmentVariable('Path','{scope}'); if($p){{ [Environment]::SetEnvironmentVariable('Path', (($p -split ';' | Where-Object {{ $_ -and $_ -ne $d }}) -join ';'), '{scope}') }}"
        );
        run(o, "powershell", &["-NoProfile", "-NonInteractive", "-Command", &script]);
        let menu = if o.system {
            PathBuf::from(env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into())).join("Microsoft\\Windows\\Start Menu\\Programs")
        } else {
            PathBuf::from(env::var("APPDATA").unwrap_or_default()).join("Microsoft\\Windows\\Start Menu\\Programs")
        };
        if !o.dry {
            let _ = fs::remove_file(menu.join("Minima.lnk"));
        }
        let hive = if o.system { "HKLM" } else { "HKCU" };
        run(o, "reg", &["delete", &format!("{hive}\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Minima"), "/f"]);
        // we may be the running uninstaller: delete ourselves after we exit
        if !o.dry {
            let me = dir.join("minima-uninstall.exe");
            use std::os::windows::process::CommandExt;
            let cmd = format!("/C ping -n 3 127.0.0.1 >nul & del /f /q \"{}\" & rmdir \"{}\"", me.display(), dir.display());
            let _ = Command::new("cmd").raw_arg(cmd).spawn();
        }
    }
    #[cfg(not(windows))]
    {
        if !o.dry {
            let apps = if o.system { PathBuf::from("/usr/share/applications") } else { home().join(".local/share/applications") };
            let _ = fs::remove_file(apps.join("minima.desktop"));
            let share = if o.system { PathBuf::from("/usr/share") } else { home().join(".local/share") };
            for size in ["256x256", "48x48"] {
                let _ = fs::remove_file(share.join("icons/hicolor").join(size).join("apps/minima.png"));
            }
        }
    }
    println!("Removed. (Your history and bookmarks were left in place.)");
}
