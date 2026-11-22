//! Word-wrapping layout: `Doc` → styled terminal lines with widget hit boxes.

use unicode_width::UnicodeWidthChar;

use crate::doc::*;
use crate::util::width;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Normal,
    H1,
    H2,
    H3,
    Link,
    Image,
    Video,
    Button,
    Field,
    Hint,
    Check,
    Select,
    Quote,
    Pre,
    Dim,
    Marker,
}

#[derive(Clone, Debug)]
pub struct Seg {
    pub text: String,
    pub style: Style,
    pub widget: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Line {
    pub segs: Vec<Seg>,
}

impl Line {
    pub fn width(&self) -> usize {
        self.segs.iter().map(|s| width(&s.text)).sum()
    }
    pub fn text(&self) -> String {
        self.segs.iter().map(|s| s.text.as_str()).collect()
    }
}

pub struct Layout {
    pub lines: Vec<Line>,
    /// (line, col) of the first cell of each widget.
    pub widget_pos: Vec<Option<(usize, usize)>>,
    /// Plain-text of each block's first line → used for "ask the page".
    pub block_starts: Vec<usize>,
}

struct Atom {
    text: String,
    style: Style,
    widget: Option<usize>,
    space_before: bool,
    unbreakable: bool,
}

fn field_width(avail: usize, label: &str) -> usize {
    let want = if label.to_lowercase().contains("search") { 40 } else { 28 };
    want.min(avail.saturating_sub(4)).max(8)
}

fn widget_atoms(doc: &Doc, wid: usize, avail: usize, editing: Option<usize>, out: &mut Vec<Atom>, space_before: bool) {
    let w = &doc.widgets[wid];
    match &w.kind {
        WidgetKind::Link { .. } => {
            let mut first = true;
            for word in w.label.split_whitespace() {
                out.push(Atom { text: word.to_string(), style: Style::Link, widget: Some(wid), space_before: if first { space_before } else { true }, unbreakable: false });
                first = false;
            }
        }
        WidgetKind::Image { .. } => {
            let label = format!("▣ {}", crate::util::truncate_width(&w.label, avail.saturating_sub(6).max(10)));
            out.push(Atom { text: label, style: Style::Image, widget: Some(wid), space_before, unbreakable: true });
        }
        WidgetKind::Video { .. } => {
            let label = format!("▶ {}", crate::util::truncate_width(&w.label, avail.saturating_sub(6).max(10)));
            out.push(Atom { text: label, style: Style::Video, widget: Some(wid), space_before, unbreakable: true });
        }
        WidgetKind::Button { .. } => {
            let label = format!("[ {} ]", crate::util::truncate_width(&w.label, avail.saturating_sub(6).max(4)));
            out.push(Atom { text: label, style: Style::Button, widget: Some(wid), space_before, unbreakable: true });
        }
        WidgetKind::Check { checked, radio, .. } => {
            let t = match (radio, checked) {
                (false, true) => "[x]",
                (false, false) => "[ ]",
                (true, true) => "(•)",
                (true, false) => "( )",
            };
            out.push(Atom { text: t.to_string(), style: Style::Check, widget: Some(wid), space_before, unbreakable: true });
        }
        WidgetKind::Select { options, selected, .. } => {
            let cur = options.get(*selected).map(|o| o.1.as_str()).unwrap_or("");
            let t = format!("‹ {} ▾›", crate::util::truncate_width(cur, avail.saturating_sub(8).max(6)));
            out.push(Atom { text: t, style: Style::Select, widget: Some(wid), space_before, unbreakable: true });
        }
        WidgetKind::Text { value, password, multiline, .. } => {
            let fw = field_width(avail, &w.label);
            let shown: String = if *password { "•".repeat(value.chars().count()) } else { value.replace('\n', "⏎") };
            let is_edit = editing == Some(wid);
            let (inner, style) = if shown.is_empty() && !is_edit && w.hint {
                (hint_pad(&w.label, fw), Style::Hint)
            } else {
                let cursor = w.cursor.min(shown.chars().count());
                (window_pad(&shown, cursor, fw, is_edit), Style::Field)
            };
            let _ = multiline;
            out.push(Atom { text: format!("[{}]", inner), style, widget: Some(wid), space_before, unbreakable: true });
        }
    }
}

fn hint_pad(label: &str, fw: usize) -> String {
    let l = crate::util::truncate_width(label, fw);
    let pad = fw.saturating_sub(width(&l));
    format!("{}{}", l, " ".repeat(pad))
}

/// Show the part of `s` around the cursor, padded with `_` to `fw` cells.
fn window_pad(s: &str, cursor: usize, fw: usize, editing: bool) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut start = 0;
    // scroll so the cursor is visible
    let mut w = 0;
    let mut i = cursor.min(chars.len());
    while i > 0 {
        let cw = chars[i - 1].width().unwrap_or(0);
        if w + cw >= fw {
            break;
        }
        w += cw;
        i -= 1;
    }
    if chars.len() > 0 && (cursor >= chars.len() || width(s) > fw) {
        start = i;
    }
    if !editing {
        start = 0;
    }
    let mut out = String::new();
    let mut used = 0;
    for &c in &chars[start..] {
        let cw = c.width().unwrap_or(0);
        if used + cw > fw {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push_str(&"_".repeat(fw.saturating_sub(used)));
    out
}

/// Wrap atoms into rows of segs. `first_indent`/`rest_indent` are leading spaces already
/// accounted in `max_w` by the caller via prefix segs.
fn wrap(atoms: Vec<Atom>, max_w: usize, base_style: Style) -> Vec<Vec<Seg>> {
    let mut rows: Vec<Vec<Seg>> = vec![Vec::new()];
    let mut cur_w = 0usize;
    let push_seg = |row: &mut Vec<Seg>, text: String, style: Style, widget: Option<usize>| {
        if let Some(last) = row.last_mut() {
            if last.style == style && last.widget == widget {
                last.text.push_str(&text);
                return;
            }
        }
        row.push(Seg { text, style, widget });
    };
    let mut prev_widget: Option<usize> = None;
    for a in atoms {
        // forced line break marker
        if a.text == "\n" {
            rows.push(Vec::new());
            cur_w = 0;
            prev_widget = None;
            continue;
        }
        let aw = width(&a.text);
        let sp = if a.space_before && cur_w > 0 { 1 } else { 0 };
        if cur_w + sp + aw > max_w && cur_w > 0 {
            rows.push(Vec::new());
            cur_w = 0;
            prev_widget = None;
        }
        let sp = if a.space_before && cur_w > 0 { 1 } else { 0 };
        let row = rows.last_mut().unwrap();
        if sp == 1 {
            // keep a link continuous across its own words
            let (st, wd) = if a.widget.is_some() && a.widget == prev_widget { (a.style, a.widget) } else { (base_style, None) };
            push_seg(row, " ".to_string(), st, wd);
            cur_w += 1;
        }
        if aw > max_w && !a.unbreakable || (aw > max_w && max_w > 0) {
            // hard-split very long tokens
            let mut chunk = String::new();
            let mut cw_acc = 0;
            for ch in a.text.chars() {
                let cw = ch.width().unwrap_or(0);
                if cur_w + cw_acc + cw > max_w {
                    push_seg(rows.last_mut().unwrap(), std::mem::take(&mut chunk), a.style, a.widget);
                    rows.push(Vec::new());
                    cur_w = 0;
                    cw_acc = 0;
                }
                chunk.push(ch);
                cw_acc += cw;
            }
            if !chunk.is_empty() {
                push_seg(rows.last_mut().unwrap(), chunk, a.style, a.widget);
                cur_w += cw_acc;
            }
        } else {
            push_seg(rows.last_mut().unwrap(), a.text, a.style, a.widget);
            cur_w += aw;
        }
        prev_widget = a.widget;
    }
    while rows.len() > 1 && rows.last().map_or(false, |r| r.is_empty()) {
        rows.pop();
    }
    rows
}

fn inline_atoms(doc: &Doc, inl: &[Inline], avail: usize, editing: Option<usize>, style: Style) -> Vec<Atom> {
    let mut atoms = Vec::new();
    let mut pending_space = false;
    for i in inl {
        match i {
            Inline::Text(t) => {
                if t.starts_with(' ') {
                    pending_space = true;
                }
                let mut first = true;
                for word in t.split_whitespace() {
                    atoms.push(Atom { text: word.to_string(), style, widget: None, space_before: !first || pending_space || atoms.is_empty() && false, unbreakable: false });
                    pending_space = false;
                    first = false;
                }
                if t.ends_with(' ') {
                    pending_space = true;
                }
            }
            Inline::Widget(w) => {
                let sp = pending_space || !atoms.is_empty();
                widget_atoms(doc, *w, avail, editing, &mut atoms, sp);
                pending_space = true;
            }
            Inline::Break => {
                atoms.push(Atom { text: "\n".into(), style, widget: None, space_before: false, unbreakable: true });
                pending_space = false;
            }
        }
    }
    // the first atom never needs a leading space
    if let Some(a) = atoms.first_mut() {
        a.space_before = false;
    }
    atoms
}

pub fn layout(doc: &Doc, term_width: usize, editing: Option<usize>) -> Layout {
    let total = term_width.saturating_sub(2).clamp(20, 110);
    let mut lines: Vec<Line> = Vec::new();
    let mut widget_pos: Vec<Option<(usize, usize)>> = vec![None; doc.widgets.len()];
    let mut block_starts = Vec::with_capacity(doc.blocks.len());
    let margin = (term_width.saturating_sub(total) / 2).max(1);

    // record widget positions + push a row with prefix segs
    let mut emit = |lines: &mut Vec<Line>, prefix: Vec<Seg>, row: Vec<Seg>| {
        let mut col = margin + prefix.iter().map(|s| width(&s.text)).sum::<usize>();
        let line_idx = lines.len();
        for s in &row {
            if let Some(w) = s.widget {
                if widget_pos[w].is_none() {
                    widget_pos[w] = Some((line_idx, col));
                }
            }
            col += width(&s.text);
        }
        let mut segs = Vec::with_capacity(prefix.len() + row.len() + 1);
        segs.push(Seg { text: " ".repeat(margin), style: Style::Normal, widget: None });
        segs.extend(prefix);
        segs.extend(row);
        lines.push(Line { segs });
    };
    let blank = |lines: &mut Vec<Line>| {
        if !lines.last().map_or(true, |l| l.segs.is_empty() || l.text().trim().is_empty()) {
            lines.push(Line::default());
        }
    };
    let pfx = |s: &str, style: Style| Seg { text: s.to_string(), style, widget: None };
    let mut prev_item = false;

    for b in &doc.blocks {
        block_starts.push(lines.len());
        let is_item = matches!(b, Block::Item { .. });
        if prev_item && !is_item {
            blank(&mut lines);
        }
        prev_item = is_item;
        match b {
            Block::Heading { level, inl } => {
                blank(&mut lines);
                let (style, mark) = match level {
                    1 => (Style::H1, "# "),
                    2 => (Style::H2, "## "),
                    _ => (Style::H3, "### "),
                };
                let atoms = inline_atoms(doc, inl, total, editing, style);
                let mw = width(mark);
                let rows = wrap(atoms, total.saturating_sub(mw).max(10), style);
                for (i, r) in rows.into_iter().enumerate() {
                    let p = if i == 0 { vec![pfx(mark, style)] } else { vec![pfx(&" ".repeat(mw), style)] };
                    emit(&mut lines, p, r);
                }
                if *level == 1 {
                    emit(&mut lines, vec![], vec![pfx(&"═".repeat(total.min(60)), Style::Dim)]);
                }
            }
            Block::Para(inl) => {
                let atoms = inline_atoms(doc, inl, total, editing, Style::Normal);
                for r in wrap(atoms, total, Style::Normal) {
                    emit(&mut lines, vec![], r);
                }
                blank(&mut lines);
            }
            Block::Item { depth, marker, inl } => {
                let ind = *depth as usize * 2;
                let mw = width(marker) + 1;
                let atoms = inline_atoms(doc, inl, total, editing, Style::Normal);
                let rows = wrap(atoms, total.saturating_sub(ind + mw).max(10), Style::Normal);
                for (i, r) in rows.into_iter().enumerate() {
                    let p = if i == 0 {
                        vec![pfx(&" ".repeat(ind), Style::Normal), pfx(&format!("{marker} "), Style::Marker)]
                    } else {
                        vec![pfx(&" ".repeat(ind + mw), Style::Normal)]
                    };
                    emit(&mut lines, p, r);
                }
            }
            Block::Quote { depth, inl } => {
                let bars = "│ ".repeat((*depth as usize).min(4));
                let bw = width(&bars);
                let atoms = inline_atoms(doc, inl, total, editing, Style::Quote);
                for r in wrap(atoms, total.saturating_sub(bw).max(10), Style::Quote) {
                    emit(&mut lines, vec![pfx(&bars, Style::Dim)], r);
                }
                blank(&mut lines);
            }
            Block::Pre(text) => {
                for l in text.lines() {
                    let l = l.replace('\t', "    ");
                    // hard-wrap long code lines
                    let mut cur = String::new();
                    let mut w = 0;
                    for ch in l.chars() {
                        let cw = ch.width().unwrap_or(0);
                        if w + cw > total.saturating_sub(2) {
                            emit(&mut lines, vec![pfx("  ", Style::Pre)], vec![pfx(&std::mem::take(&mut cur), Style::Pre)]);
                            w = 0;
                        }
                        cur.push(ch);
                        w += cw;
                    }
                    emit(&mut lines, vec![pfx("  ", Style::Pre)], vec![pfx(&cur, Style::Pre)]);
                }
                blank(&mut lines);
            }
            Block::Row(cells) => {
                let n = cells.len();
                let sep = " │ ";
                // column widths: natural width of each cell, shrinking the widest until it fits
                let avail = total.saturating_sub(3 * (n - 1));
                let mut cws: Vec<usize> = cells
                    .iter()
                    .map(|c| {
                        let atoms = inline_atoms(doc, c, total, editing, Style::Normal);
                        atoms.iter().map(|a| width(&a.text) + a.space_before as usize).sum::<usize>().max(1)
                    })
                    .collect();
                while cws.iter().sum::<usize>() > avail {
                    let (imax, vmax) = cws.iter().cloned().enumerate().max_by_key(|x| x.1).unwrap();
                    if vmax <= 10 {
                        break;
                    }
                    cws[imax] -= 1;
                }
                let cw = *cws.iter().min().unwrap_or(&1);
                if cws.iter().sum::<usize>() > avail || n > 8 || (cws.iter().filter(|w| **w >= 10).count() >= 3 && cw < 10 && n > 4) {
                    // too narrow: flow all cells into one paragraph
                    let mut inl: Vec<Inline> = Vec::new();
                    for (i, c) in cells.iter().enumerate() {
                        if i > 0 {
                            inl.push(Inline::Text(" │ ".into()));
                        }
                        inl.extend(c.iter().cloned());
                    }
                    let atoms = inline_atoms(doc, &inl, total, editing, Style::Normal);
                    for r in wrap(atoms, total, Style::Normal) {
                        emit(&mut lines, vec![], r);
                    }
                } else {
                    let wrapped: Vec<Vec<Vec<Seg>>> = cells
                        .iter()
                        .enumerate()
                        .map(|(i, c)| wrap(inline_atoms(doc, c, cws[i], editing, Style::Normal), cws[i], Style::Normal))
                        .collect();
                    let h = wrapped.iter().map(|c| c.len()).max().unwrap_or(1);
                    for y in 0..h {
                        let mut row: Vec<Seg> = Vec::new();
                        for (ci, c) in wrapped.iter().enumerate() {
                            let segs = c.get(y).cloned().unwrap_or_default();
                            let used: usize = segs.iter().map(|s| width(&s.text)).sum();
                            row.extend(segs);
                            if ci + 1 < n {
                                row.push(pfx(&" ".repeat(cws[ci].saturating_sub(used)), Style::Normal));
                                row.push(pfx(sep, Style::Dim));
                            }
                        }
                        emit(&mut lines, vec![], row);
                    }
                }
            }
            Block::Rule => {
                blank(&mut lines);
                emit(&mut lines, vec![], vec![pfx(&"─".repeat(total.min(72)), Style::Dim)]);
                blank(&mut lines);
            }
        }
    }
    while lines.last().map_or(false, |l| l.text().trim().is_empty()) {
        lines.pop();
    }
    Layout { lines, widget_pos, block_starts }
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    #[test]
    fn wraps_and_positions() {
        let mut d = Doc::empty(Url::parse("https://x.test/").unwrap());
        let w = d.push_widget(Widget::new(WidgetKind::Link { href: "https://x.test/a".into() }, "a link".into()));
        d.blocks.push(Block::Para(vec![
            Inline::Text("hello world this is a fairly long line of text that should wrap ".into()),
            Inline::Widget(w),
            Inline::Text(" tail".into()),
        ]));
        let l = layout(&d, 30, None);
        assert!(l.lines.len() >= 2);
        assert!(l.lines.iter().all(|ln| ln.width() <= 30));
        assert!(l.widget_pos[w].is_some());
    }
}
