//! The reference sections of `docs/api.md`, rendered from the binding
//! descriptors in `kuula_lua::api` and spliced between HTML comment
//! markers. The prose around the markers is handwritten and untouched.
//!
//! A section starts with `<!-- generated: <key> -->` on its own line
//! and ends with `<!-- /generated -->`; `<key>` is a [`Group::key`].
//! Rendering is a pure function of the inventory, so `--check` can
//! compare in memory and `--write` produces the same bytes every time.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use kuula_lua::api::{Binding, Group, INTERNAL_MARKER};

/// Start of a generated section, followed by the key and ` -->`.
pub const BEGIN: &str = "<!-- generated: ";
/// End of a generated section.
pub const END: &str = "<!-- /generated -->";

/// Line width the bullets wrap at.
const WIDTH: usize = 72;

/// Every section body, keyed by group, from an inventory that has
/// passed `kuula_lua::api::problems`.
pub fn sections(inventory: &[&Binding]) -> BTreeMap<&'static str, String> {
    Group::ALL
        .iter()
        .map(|&g| (g.key(), section(g, inventory)))
        .collect()
}

/// A binding's name as the bullets show it.
fn display(b: &Binding) -> String {
    if b.name.starts_with("__") {
        b.sigs[0]
            .call
            .split('(')
            .next()
            .unwrap_or(b.sigs[0].call)
            .to_string()
    } else {
        b.qualified()
    }
}

/// Whether a table for `group` shows the returns column: the standard
/// library's entries return what Lua's do.
fn has_returns(group: Group) -> bool {
    !matches!(group, Group::Stdlib | Group::Numeric)
}

fn section(group: Group, inventory: &[&Binding]) -> String {
    let mut out = String::new();
    let call = match group {
        Group::BufMethods => "method",
        _ => "call",
    };
    if has_returns(group) {
        out.push_str(&format!("| {call} | returns | cycles |\n|---|---|---|\n"));
    } else {
        out.push_str(&format!("| {call} | cycles |\n|---|---|\n"));
    }
    for b in inventory {
        for sig in b.sigs {
            if sig.group.unwrap_or(b.group) != group {
                continue;
            }
            let cycles = sig.price.unwrap_or(b.price).render();
            let call = format!("`{}`", sig.call);
            if has_returns(group) {
                writeln!(out, "| {call} | {} | {cycles} |", sig.returns).unwrap();
            } else {
                writeln!(out, "| {call} | {cycles} |").unwrap();
            }
        }
    }
    let mut bullets = Vec::new();
    for b in inventory {
        if b.group != group {
            continue;
        }
        let mut text = String::new();
        if !b.doc.is_empty() {
            text.push_str(b.doc);
        }
        if !b.defaults.is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            let list: Vec<String> = b
                .defaults
                .iter()
                .map(|(a, d)| format!("`{a}` = {d}"))
                .collect();
            text.push_str(&format!("Defaults: {}.", list.join(", ")));
        }
        if !b.errors.is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            let list: Vec<String> = b.errors.iter().map(|e| format!("`{e}`")).collect();
            text.push_str(&format!("Errors: {}.", list.join(", ")));
        }
        if !text.is_empty() {
            bullets.push(wrap(&format!("- `{}`: {text}", display(b))));
        }
    }
    if !bullets.is_empty() {
        out.push('\n');
        for b in bullets {
            out.push_str(&b);
            out.push('\n');
        }
    }
    out
}

/// Word-wrap a bullet at [`WIDTH`], continuation lines indented.
fn wrap(text: &str) -> String {
    let mut out = String::new();
    let mut line = String::new();
    for word in text.split(' ') {
        if line.is_empty() {
            line.push_str(word);
        } else if line.len() + 1 + word.len() > WIDTH {
            out.push_str(&line);
            out.push('\n');
            line = format!("  {word}");
        } else {
            line.push(' ');
            line.push_str(word);
        }
    }
    out.push_str(&line);
    out
}

/// Whether a line opens a generated section, and its key.
fn begin_key(line: &str) -> Option<&str> {
    line.trim_end()
        .strip_prefix(BEGIN)
        .and_then(|rest| rest.strip_suffix(" -->"))
}

/// `doc` with every generated section replaced by its rendering.
/// Every key in `sections` must have a marker in `doc` and every
/// marker a key.
pub fn splice(doc: &str, sections: &BTreeMap<&'static str, String>) -> Result<String, String> {
    let mut out = String::new();
    let mut seen = Vec::new();
    let mut lines = doc.lines().peekable();
    while let Some(line) = lines.next() {
        out.push_str(line);
        out.push('\n');
        let Some(key) = begin_key(line) else {
            continue;
        };
        let body = sections
            .get(key)
            .ok_or_else(|| format!("unknown generated section {key:?}"))?;
        if seen.contains(&key) {
            return Err(format!("generated section {key:?} appears twice"));
        }
        seen.push(key);
        let mut closed = false;
        for inner in lines.by_ref() {
            if inner.trim_end() == END {
                closed = true;
                break;
            }
            if begin_key(inner).is_some() {
                return Err(format!("generated section {key:?} is not closed"));
            }
        }
        if !closed {
            return Err(format!("generated section {key:?} is not closed"));
        }
        out.push_str(body);
        out.push_str(END);
        out.push('\n');
    }
    for key in sections.keys() {
        if !seen.contains(key) {
            return Err(format!("the document has no `{BEGIN}{key} -->` section"));
        }
    }
    if !doc.ends_with('\n') {
        out.pop();
    }
    Ok(out)
}

/// The document as the generator would write it, or why it cannot.
pub fn render(doc: &str) -> Result<String, String> {
    let inventory = kuula_lua::api::inventory();
    let problems = kuula_lua::api::problems(&inventory);
    if !problems.is_empty() {
        return Err(format!("descriptor problems:\n  {}", problems.join("\n  ")));
    }
    let sections = sections(&inventory);
    for (key, body) in &sections {
        if body.contains(INTERNAL_MARKER) {
            return Err(format!("section {key:?} carries {INTERNAL_MARKER}"));
        }
    }
    splice(doc, &sections)
}

/// `Ok` when `doc` already holds what [`render`] produces; otherwise
/// a line diff, `-` for what the document has and `+` for what the
/// descriptors say.
pub fn check(doc: &str) -> Result<(), String> {
    let fresh = render(doc)?;
    if fresh == doc {
        return Ok(());
    }
    Err(format!(
        "the generated sections are stale; run `cargo run -p kuula-apidoc -- --write`\n{}",
        diff(doc, &fresh)
    ))
}

/// A minimal line diff (longest common subsequence).
pub fn diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out = String::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            writeln!(out, "-{}", a[i]).unwrap();
            i += 1;
        } else {
            writeln!(out, "+{}", b[j]).unwrap();
            j += 1;
        }
    }
    for line in &a[i..] {
        writeln!(out, "-{line}").unwrap();
    }
    for line in &b[j..] {
        writeln!(out, "+{line}").unwrap();
    }
    out
}

/// `docs/api.md` relative to this crate, the default target.
pub fn default_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("api.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splice_replaces_bodies_and_reports_marker_problems() {
        let mut sections = BTreeMap::new();
        sections.insert("a", "body a\n".to_string());
        let doc = "intro\n<!-- generated: a -->\nold\n<!-- /generated -->\noutro\n";
        assert_eq!(
            splice(doc, &sections).unwrap(),
            "intro\n<!-- generated: a -->\nbody a\n<!-- /generated -->\noutro\n"
        );
        let err = splice("intro\n", &sections).unwrap_err();
        assert!(err.contains("no `<!-- generated: a -->`"), "{err}");
        let err = splice("<!-- generated: b -->\n<!-- /generated -->\n", &sections).unwrap_err();
        assert!(err.contains("unknown"), "{err}");
        let err = splice("<!-- generated: a -->\nx\n", &sections).unwrap_err();
        assert!(err.contains("not closed"), "{err}");
    }

    #[test]
    fn diff_marks_changed_lines() {
        let d = diff("a\nb\nc\n", "a\nx\nc\n");
        assert_eq!(d, "-b\n+x\n");
    }

    #[test]
    fn wrap_keeps_lines_under_the_width() {
        let text = format!("- `x`: {}", "word ".repeat(40));
        for line in wrap(text.trim()).lines() {
            assert!(line.len() <= WIDTH, "{line}");
        }
    }
}
