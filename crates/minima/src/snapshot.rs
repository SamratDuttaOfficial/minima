//! Headless rendering for tests: run the real UI against a virtual screen.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

use crate::app::App;

/// Interpret the UI's escape output into a plain-text grid. With `marks`,
/// reverse-video runs (focus, bars) are wrapped in ⟦ ⟧.
pub fn screen_to_text(s: &str, w: usize, h: usize, marks: bool) -> String {
    let mut grid: Vec<Vec<(char, bool)>> = vec![vec![(' ', false); w]; h];
    let (mut r, mut c) = (0usize, 0usize);
    let mut rev = false;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '\x1b' {
            i += 1;
            match chars.get(i) {
                Some('[') => {
                    i += 1;
                    let mut params = String::new();
                    while i < chars.len() && !chars[i].is_ascii_alphabetic() {
                        params.push(chars[i]);
                        i += 1;
                    }
                    let fin = chars.get(i).cloned().unwrap_or('m');
                    let nums: Vec<usize> = params.trim_start_matches('?').split(';').filter_map(|p| p.parse().ok()).collect();
                    match fin {
                        'H' => {
                            r = nums.first().cloned().unwrap_or(1).saturating_sub(1).min(h - 1);
                            c = nums.get(1).cloned().unwrap_or(1).saturating_sub(1).min(w - 1);
                        }
                        'K' => {
                            let mode = nums.first().cloned().unwrap_or(0);
                            let range = if mode == 2 { 0..w } else { c..w };
                            for x in range {
                                grid[r][x] = (' ', false);
                            }
                        }
                        'D' => c = c.saturating_sub(nums.first().cloned().unwrap_or(1)),
                        'J' => grid = vec![vec![(' ', false); w]; h],
                        'm' => {
                            if nums.is_empty() {
                                rev = false;
                            }
                            for n in &nums {
                                match n {
                                    0 | 27 => rev = false,
                                    7 => rev = true,
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Some(']') => {
                    while i < chars.len() && chars[i] != '\x07' {
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
            continue;
        }
        let cw = ch.width().unwrap_or(0);
        if cw > 0 && r < h && c < w {
            grid[r][c] = (ch, rev);
            if cw == 2 && c + 1 < w {
                grid[r][c + 1] = ('\u{0}', rev);
            }
            c += cw;
        }
        i += 1;
    }
    let mut out = String::new();
    for row in grid {
        let mut line = String::new();
        let mut in_rev = false;
        for (ch, rv) in row {
            if marks && rv != in_rev {
                line.push(if rv { '⟦' } else { '⟧' });
                in_rev = rv;
            }
            if ch != '\u{0}' {
                line.push(ch);
            }
        }
        if in_rev && marks {
            line.push('⟧');
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn parse_token(t: &str) -> Option<KeyEvent> {
    let none = KeyModifiers::NONE;
    let (code, m) = match t {
        "Tab" => (KeyCode::Tab, none),
        "S-Tab" => (KeyCode::BackTab, KeyModifiers::SHIFT),
        "Enter" => (KeyCode::Enter, none),
        "Esc" => (KeyCode::Esc, none),
        "Up" => (KeyCode::Up, none),
        "Down" => (KeyCode::Down, none),
        "Left" => (KeyCode::Left, none),
        "Right" => (KeyCode::Right, none),
        "A-Left" => (KeyCode::Left, KeyModifiers::ALT),
        "A-Right" => (KeyCode::Right, KeyModifiers::ALT),
        "PgDn" => (KeyCode::PageDown, none),
        "PgUp" => (KeyCode::PageUp, none),
        "Home" => (KeyCode::Home, none),
        "End" => (KeyCode::End, none),
        "Bksp" => (KeyCode::Backspace, none),
        "Del" => (KeyCode::Delete, none),
        "Space" => (KeyCode::Char(' '), none),
        _ if t.starts_with("C-") && t.len() == 3 => (KeyCode::Char(t.chars().nth(2)?), KeyModifiers::CONTROL),
        _ => return None,
    };
    Some(KeyEvent::new(code, m))
}

/// Apply a key script such as `g<Enter>rust<Enter><Tab><Tab><Enter>`.
pub fn run_keys(app: &mut App, spec: &str) {
    let chars: Vec<char> = spec.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            if let Some(end) = chars[i..].iter().position(|c| *c == '>') {
                let name: String = chars[i + 1..i + end].iter().collect();
                if let Some(k) = parse_token(&name) {
                    app.test_key(k);
                    i += end + 1;
                    continue;
                }
            }
        }
        let c = chars[i];
        let mods = if c.is_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
        app.test_key(KeyEvent::new(KeyCode::Char(c), mods));
        i += 1;
    }
}

pub fn frame(app: &mut App, marks: bool) -> String {
    let (w, h) = (app.size.0 as usize, app.size.1 as usize);
    let raw = app.render();
    let mut s = screen_to_text(&raw, w, h, marks);
    s.push_str(&format!("{}\n", "─".repeat(w)));
    let _ = Duration::from_secs(0);
    s
}
