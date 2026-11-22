//! Minima — a fast, compiled, terminal web browser.

mod app;
mod ascii;
mod command;
mod config;
mod doc;
mod extract;
mod fetch;
mod history;
mod layout;
mod load;
mod media;
mod nlp;
mod pages;
mod snapshot;
mod util;
mod youtube;

use std::io::Write;

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const HELP: &str = "\
Minima — the web, stripped down, in your terminal.

USAGE
    minima [URL | search words]        open the browser (default: your home page)
    minima --dump URL [--width N]      print the stripped page as text and exit
    minima --image FILE|URL [--mode ascii|blocks|braille] [--no-color] [--cols N]
                                       print an image as terminal art and exit
    minima --config                    print the config file path
    minima --version | --help

INSIDE THE BROWSER
    Tab / arrows  select links and fields      Enter  activate
    g  go to URL or search                     :      ask Minima (\"click sign in\")
    / n N  find in page                        L      list links and controls
    i I  view image(s) as ASCII art            p      play video   d  download
    H B b  history / bookmarks / bookmark      ?      full help    q  quit
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = config::Config::load();

    match args.first().map(|s| s.as_str()) {
        Some("--help") | Some("-h") => {
            print!("{HELP}");
            return;
        }
        Some("--version") | Some("-V") => {
            println!("minima {VERSION}");
            return;
        }
        Some("--config") => {
            cfg.ensure_default_file();
            println!("{}", cfg.config_path.display());
            return;
        }
        Some("--dump") => return dump(&cfg, &args[1..]),
        Some("--image") => return image_cmd(&cfg, &args[1..]),
        Some("--snapshot") => return snapshot_cmd(cfg, &args[1..]),
        _ => {}
    }

    let start = if args.is_empty() { None } else { Some(args.join(" ")) };

    let mut out = std::io::stdout();
    if enable_raw_mode().is_err() {
        eprintln!("minima: needs an interactive terminal (try --dump URL for plain output)");
        std::process::exit(1);
    }
    let _ = execute!(out, EnterAlternateScreen, EnableMouseCapture);
    let _ = out.write_all(b"\x1b[?25l");
    let _ = out.flush();

    // restore the terminal even if we panic (release builds abort on panic)
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        default_hook(info);
    }));

    let mut app = app::App::new(cfg);
    app.run(start);
    restore();
}

fn restore() {
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x1b]50;#+0\x07\x1b[0m\x1b[?25h");
    let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    let _ = out.flush();
}

fn arg_val(args: &[String], key: &str) -> Option<String> {
    args.iter().position(|a| a == key).and_then(|i| args.get(i + 1)).cloned()
}

fn dump(cfg: &config::Config, args: &[String]) {
    let Some(target) = args.iter().find(|a| !a.starts_with("--") && args.iter().position(|x| x == *a).map_or(true, |i| i == 0 || args[i - 1] != "--width")) else {
        eprintln!("usage: minima --dump URL [--width N]");
        std::process::exit(2);
    };
    let width: usize = arg_val(args, "--width").and_then(|w| w.parse().ok()).unwrap_or(100);
    let input = util::resolve_input(target, &cfg.search);
    let url = match url::Url::parse(&input) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("bad URL: {e}");
            std::process::exit(2);
        }
    };
    let fetcher = fetch::Fetcher::new(&cfg.user_agent);
    let search = cfg.search.clone();
    let handle = std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(move || load::load(&fetcher, &url, None, &search))
        .unwrap();
    match handle.join().unwrap() {
        Ok(load::Loaded::Doc(doc)) => {
            let lay = layout::layout(&doc, width + 2, None);
            println!("# {}  ({}, {} bytes → {} lines, {} blocks stripped)", doc.title, doc.kind.name(), doc.raw_bytes, lay.lines.len(), doc.stripped);
            println!("# {}", doc.url);
            println!();
            for l in &lay.lines {
                println!("{}", l.text().trim_end());
            }
            let links = doc.widgets.iter().filter(|w| matches!(w.kind, doc::WidgetKind::Link { .. })).count();
            let imgs = doc.widgets.iter().filter(|w| matches!(w.kind, doc::WidgetKind::Image { .. })).count();
            println!();
            println!("# {} links, {} images, {} forms", links, imgs, doc.forms.len());
        }
        Ok(_) => {
            eprintln!("not an HTML page (image / video / file)");
            std::process::exit(3);
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

fn image_cmd(cfg: &config::Config, args: &[String]) {
    let Some(src) = args.iter().find(|a| !a.starts_with("--") && args.iter().position(|x| x == *a).map_or(true, |i| i == 0 || !matches!(args[i - 1].as_str(), "--mode" | "--cols"))) else {
        eprintln!("usage: minima --image FILE|URL [--mode ascii|blocks|braille] [--no-color] [--cols N]");
        std::process::exit(2);
    };
    let mode = match arg_val(args, "--mode").as_deref() {
        Some("ascii") => config::ImageMode::Ascii,
        Some("blocks") => config::ImageMode::Blocks,
        Some("braille") | None => config::ImageMode::Braille,
        Some(o) => {
            eprintln!("unknown mode {o}");
            std::process::exit(2);
        }
    };
    let color = !args.iter().any(|a| a == "--no-color") && cfg.color;
    let (tw, th) = crossterm::terminal::size().unwrap_or((100, 40));
    let cols: usize = arg_val(args, "--cols").and_then(|c| c.parse().ok()).unwrap_or(tw as usize);
    let bytes = if src.starts_with("http://") || src.starts_with("https://") {
        let f = fetch::Fetcher::new(&cfg.user_agent);
        match url::Url::parse(src).map_err(|e| e.to_string()).and_then(|u| f.get(&u)) {
            Ok(r) => r.body,
            Err(e) => {
                eprintln!("fetch failed: {e}");
                std::process::exit(1);
            }
        }
    } else {
        match std::fs::read(src) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{src}: {e}");
                std::process::exit(1);
            }
        }
    };
    match image::load_from_memory(&bytes) {
        Ok(img) => {
            for l in ascii::render_image(&img, mode, color, cols, (th as usize).saturating_sub(1).max(10)) {
                println!("{l}");
            }
        }
        Err(e) => {
            eprintln!("cannot decode image: {e}");
            std::process::exit(1);
        }
    }
}

/// `minima --snapshot URL [--keys SCRIPT] [--size WxH] [--marks]`
/// Drives the real UI headlessly and prints the screen. `<Snap>` in the key
/// script prints an intermediate frame. Used for testing.
fn snapshot_cmd(cfg: config::Config, args: &[String]) {
    let target = args.first().cloned().unwrap_or_else(|| "about:home".into());
    let keys = arg_val(args, "--keys").unwrap_or_default();
    let marks = args.iter().any(|a| a == "--marks");
    let (w, h) = arg_val(args, "--size")
        .and_then(|s| s.split_once('x').map(|(a, b)| (a.parse().unwrap_or(100), b.parse().unwrap_or(30))))
        .unwrap_or((100u16, 30u16));
    let handle = std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(move || {
            let mut app = app::App::new_headless(cfg, (w, h));
            if target.starts_with("about:") {
                app.navigate(&target, None);
            } else {
                app.test_open(&target);
            }
            for (i, part) in keys.split("<Snap>").enumerate() {
                if i > 0 {
                    println!("{}", snapshot::frame(&mut app, marks));
                }
                snapshot::run_keys(&mut app, part);
            }
            println!("{}", snapshot::frame(&mut app, marks));
            println!("status: {}", app.status_text());
        })
        .unwrap();
    let _ = handle.join();
}
