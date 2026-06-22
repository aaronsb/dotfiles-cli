//! `dotfiles show <name>` — one entry in full.
//!
//! The human counterpart to `status --format json`, which already serializes the
//! whole spec tree: this renders `why` prose, the structured `spec` (ADR-006),
//! any unrecognized `spec.*` keys (surfaced, never dropped), and the live deploy
//! state — giving the catalog's structured layer a human face.

use crate::{Ctx, status_view, table};
use dotfiles_core::{Requires, State};

/// Render one entry's rationale, spec, unrecognized keys, and deploy state.
pub fn run(ctx: &Ctx, app: &str) -> anyhow::Result<()> {
    let manifest = ctx.load()?;
    let state = State::derive(&manifest, &ctx.repo_root, &ctx.home);
    let es = state
        .entries
        .iter()
        .find(|es| es.entry.name == app)
        .ok_or_else(|| anyhow::anyhow!("no managed dotfile named '{app}' — try `dotfiles list`"))?;
    let e = &es.entry;

    // Header: name → ~/target [status], reusing status's label vocabulary.
    let (label, color) = status_view(&es.status, e.enabled);
    println!(
        "{}  →  ~/{}   [{}]",
        table::paint(&e.name, table::BOLD),
        e.target,
        table::paint(label, color),
    );

    if let Some(why) = &e.why {
        field("why", why);
    }

    match &e.spec {
        None => {
            // A bare entry is valid (ADR-006): nothing structured to render.
            println!("{}", table::paint("  (no structured spec)", table::DIM));
        }
        Some(spec) => {
            if let Some(s) = &spec.summary {
                field("summary", s);
            }
            if let Some(c) = &spec.concern {
                field("concern", c);
            }
            if let Some(p) = &spec.platform {
                field("platform", p);
            }
            field_list("tags", &spec.tags);
            field_list("provides", &spec.provides);
            field_list("depends", &spec.depends);
            if let Some(req) = &spec.requires {
                show_requires(req);
            }
            // The ADR-006 promise: unknown keys are surfaced, not silently
            // dropped (typo-safety) nor hard-rejected (forward-compat).
            let mut unknown: Vec<(String, String)> = spec
                .extra
                .iter()
                .map(|(k, v)| (k.clone(), v.to_string()))
                .collect();
            if let Some(req) = &spec.requires {
                unknown.extend(
                    req.extra
                        .iter()
                        .map(|(k, v)| (format!("requires.{k}"), v.to_string())),
                );
            }
            for (k, v) in unknown {
                let line = format!("{k} = {}", v.trim());
                let lbl = table::paint(&format!("{:<LABEL_W$}", "⚠ unknown"), table::YELLOW);
                println!("  {lbl} {line}");
            }
        }
    }

    // Profile scope — universal vs. listed, sharing `status`'s vocabulary.
    if e.profiles.is_empty() {
        let lbl = table::paint(&format!("{:<LABEL_W$}", "profiles"), table::DIM);
        println!("  {lbl} {}", table::paint("(universal)", table::DIM));
    } else {
        field_list("profiles", &e.profiles);
    }
    Ok(())
}

/// Detail-view label column width — the longest field label plus breathing room.
const LABEL_W: usize = 9;

/// Print a `label  value` line, wrapping long values under a hanging indent that
/// lines up beneath the value column.
fn field(label: &str, value: &str) {
    let lbl = table::paint(&format!("{label:<LABEL_W$}"), table::DIM);
    let lines = wrap(value, 72);
    let indent = 2 + LABEL_W + 1;
    println!("  {lbl} {}", lines.first().map_or("", String::as_str));
    for l in lines.iter().skip(1) {
        println!("{:indent$}{l}", "");
    }
}

/// Print a list-valued field as a comma-joined line; skip it when empty.
fn field_list(label: &str, items: &[String]) {
    if !items.is_empty() {
        field(label, &items.join(", "));
    }
}

/// Render the `requires` sub-table: one `kind: items` line per non-empty kind,
/// the `requires` label on the first, blank-indented continuations after.
fn show_requires(req: &Requires) {
    let kinds = [
        ("packages", &req.packages),
        ("groups", &req.groups),
        ("binaries", &req.binaries),
        ("configs", &req.configs),
        ("entries", &req.entries),
    ];
    let lines: Vec<String> = kinds
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{k}: {}", v.join(", ")))
        .collect();
    let indent = 2 + LABEL_W + 1;
    for (i, line) in lines.iter().enumerate() {
        if i == 0 {
            let lbl = table::paint(&format!("{:<LABEL_W$}", "requires"), table::DIM);
            println!("  {lbl} {line}");
        } else {
            println!("{:indent$}{line}", "");
        }
    }
}

/// Greedy word-wrap to `width` columns; one empty line for empty input.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::wrap;

    #[test]
    fn empty_input_yields_one_empty_line() {
        assert_eq!(wrap("", 10), vec![String::new()]);
    }

    #[test]
    fn wraps_on_word_boundaries_within_width() {
        let lines = wrap("alpha beta gamma delta", 11);
        assert_eq!(lines, vec!["alpha beta", "gamma delta"]);
        assert!(lines.iter().all(|l| l.chars().count() <= 11));
    }

    #[test]
    fn a_word_longer_than_width_is_not_split() {
        // Better to overflow than to mangle a path/identifier mid-token.
        assert_eq!(wrap("supercalifragilistic", 8), vec!["supercalifragilistic"]);
    }
}
