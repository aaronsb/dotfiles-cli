//! Friendly rendering of a unified `git diff`: a single line-number gutter, a
//! `+`/`-` marker, and a muted red/green band behind removed/added rows — the
//! at-a-glance view `dotfiles diff --details` shows. Foreground text is left
//! untouched; only the row background changes, so the band reads as a highlight
//! rather than recoloring the content. Off a terminal (piped) it degrades to
//! plain, uncolored text so `dotfiles diff … | cat` stays clean.

use crate::table::{BOLD, CYAN, DIM, GREEN, RED, RESET};
use std::fmt::Write as _;
use std::io::IsTerminal;

/// Muted 256-color backgrounds — a dark red / dark green band, not the harsh
/// primary `41`/`42`.
const DEL_BG: &str = "\x1b[48;5;52m";
const ADD_BG: &str = "\x1b[48;5;22m";

/// A single rendered diff row, after line numbers are resolved.
enum Kind {
    Context,
    Added,
    Removed,
}

struct Row {
    num: usize,
    kind: Kind,
    text: String,
    /// First row after a hunk boundary (a `@@` header), to draw a separator.
    hunk_break: bool,
}

/// Render a (possibly multi-file) unified diff into the friendly view.
pub fn render(diff: &str) -> String {
    let color = std::io::stdout().is_terminal();
    let mut out = String::new();
    let mut file: Vec<&str> = Vec::new();
    for line in diff.lines() {
        if line.starts_with("diff --git") && !file.is_empty() {
            render_file(&file, color, &mut out);
            file.clear();
        }
        file.push(line);
    }
    if !file.is_empty() {
        render_file(&file, color, &mut out);
    }
    out
}

/// Render one file's section (everything from its `diff --git` to the next).
fn render_file(lines: &[&str], color: bool, out: &mut String) {
    let rows = parse_rows(lines);
    if rows.is_empty() {
        return; // pure mode/rename change with no hunks — nothing to show
    }
    let path = file_path(lines);
    let added = rows
        .iter()
        .filter(|r| matches!(r.kind, Kind::Added))
        .count();
    let removed = rows
        .iter()
        .filter(|r| matches!(r.kind, Kind::Removed))
        .count();

    // Gutter width from the widest line number; band width so every colored row
    // ends at the same column (a solid block, like the editor view).
    let numw = rows.iter().map(|r| digits(r.num)).max().unwrap_or(1);
    let bandw = rows
        .iter()
        .map(|r| numw + 3 + r.text.chars().count())
        .max()
        .unwrap_or(0);

    if color {
        let _ = writeln!(
            out,
            "{BOLD}{CYAN}▸ {path}{RESET}  {GREEN}+{added}{RESET} {RED}-{removed}{RESET}"
        );
    } else {
        let _ = writeln!(out, "▸ {path}  +{added} -{removed}");
    }

    for row in &rows {
        if row.hunk_break && !std::ptr::eq(row, &rows[0]) {
            let _ = writeln!(
                out,
                "{}",
                if color {
                    format!("{DIM}  ⋯{RESET}")
                } else {
                    "  ⋯".into()
                }
            );
        }
        let marker = match row.kind {
            Kind::Context => ' ',
            Kind::Added => '+',
            Kind::Removed => '-',
        };
        let body = format!("{:>numw$} {marker} {}", row.num, row.text);
        match (color, &row.kind) {
            (false, _) => {
                let _ = writeln!(out, "{body}");
            }
            (true, Kind::Context) => {
                // Dim only the gutter number; leave the code text at default.
                let _ = writeln!(out, "{DIM}{:>numw$}{RESET}   {}", row.num, row.text);
            }
            (true, kind) => {
                let bg = if matches!(kind, Kind::Added) {
                    ADD_BG
                } else {
                    DEL_BG
                };
                let pad = " ".repeat(bandw.saturating_sub(body.chars().count()));
                let _ = writeln!(out, "{bg}{body}{pad}{RESET}");
            }
        }
    }
    out.push('\n');
}

/// Walk a file section's lines, resolving each hunk's `@@` start numbers into a
/// per-row absolute line number (old number for removals, new number otherwise —
/// the single-column scheme).
fn parse_rows(lines: &[&str]) -> Vec<Row> {
    let mut rows = Vec::new();
    let (mut old_no, mut new_no) = (0usize, 0usize);
    let mut pending_break = false;
    for &l in lines {
        if let Some((o, n)) = parse_hunk(l) {
            old_no = o;
            new_no = n;
            pending_break = true;
            continue;
        }
        if is_header(l) {
            continue;
        }
        if let Some(rest) = l.strip_prefix('+') {
            rows.push(Row {
                num: new_no,
                kind: Kind::Added,
                text: rest.to_string(),
                hunk_break: pending_break,
            });
            new_no += 1;
        } else if let Some(rest) = l.strip_prefix('-') {
            rows.push(Row {
                num: old_no,
                kind: Kind::Removed,
                text: rest.to_string(),
                hunk_break: pending_break,
            });
            old_no += 1;
        } else {
            // Context line (leading space) or a blank line within a hunk.
            let text = l.strip_prefix(' ').unwrap_or(l).to_string();
            rows.push(Row {
                num: new_no,
                kind: Kind::Context,
                text,
                hunk_break: pending_break,
            });
            old_no += 1;
            new_no += 1;
        }
        pending_break = false;
    }
    rows
}

/// Lines that carry no content: the git plumbing headers and the
/// "no newline at end of file" marker.
fn is_header(l: &str) -> bool {
    l.starts_with("diff ")
        || l.starts_with("index ")
        || l.starts_with("--- ")
        || l.starts_with("+++ ")
        || l.starts_with("old mode ")
        || l.starts_with("new mode ")
        || l.starts_with("similarity ")
        || l.starts_with("rename ")
        || l.starts_with("new file ")
        || l.starts_with("deleted file ")
        || l.starts_with("\\ ")
}

/// The display path for a file section, from `diff --git a/<x> b/<y>` (prefer the
/// new side; fall back to the old for deletions).
fn file_path(lines: &[&str]) -> String {
    for &l in lines {
        if let Some(rest) = l.strip_prefix("diff --git ")
            && let Some((_, b)) = rest.split_once(" b/")
        {
            return b.to_string();
        }
    }
    "(unknown)".to_string()
}

/// Parse a hunk header `@@ -old[,n] +new[,n] @@ …` into its start numbers.
fn parse_hunk(l: &str) -> Option<(usize, usize)> {
    let inner = l.strip_prefix("@@ ")?;
    let inner = inner.split(" @@").next()?;
    let (old, new) = inner.split_once(' ')?;
    let old = old.strip_prefix('-')?.split(',').next()?.parse().ok()?;
    let new = new.strip_prefix('+')?.split(',').next()?.parse().ok()?;
    Some((old, new))
}

/// Decimal digit count (min 1).
fn digits(n: usize) -> usize {
    if n == 0 {
        1
    } else {
        (n as f64).log10() as usize + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "diff --git a/x.txt b/x.txt\n\
index 111..222 100644\n\
--- a/x.txt\n\
+++ b/x.txt\n\
@@ -1,3 +1,3 @@\n\
 keep one\n\
-drop me\n\
+add me\n\
 keep two\n";

    #[test]
    fn parses_numbers_and_kinds() {
        let lines: Vec<&str> = SAMPLE.lines().collect();
        let rows = parse_rows(&lines);
        // context(1), removed(2 old), added(2 new), context(3)
        assert_eq!(rows.len(), 4);
        assert!(matches!(rows[0].kind, Kind::Context));
        assert_eq!(rows[0].num, 1);
        assert!(matches!(rows[1].kind, Kind::Removed));
        assert_eq!(rows[1].num, 2);
        assert!(matches!(rows[2].kind, Kind::Added));
        assert_eq!(rows[2].num, 2);
        assert!(matches!(rows[3].kind, Kind::Context));
        assert_eq!(rows[3].num, 3);
    }

    #[test]
    fn path_prefers_new_side() {
        let lines: Vec<&str> = SAMPLE.lines().collect();
        assert_eq!(file_path(&lines), "x.txt");
    }

    #[test]
    fn plain_render_has_markers_no_ansi() {
        // color is driven by is_terminal(); under `cargo test` stdout is not a
        // tty, so render() yields the plain branch.
        let out = render(SAMPLE);
        assert!(out.contains("▸ x.txt  +1 -1"));
        assert!(out.contains("- drop me"));
        assert!(out.contains("+ add me"));
        assert!(!out.contains('\x1b'));
    }
}
