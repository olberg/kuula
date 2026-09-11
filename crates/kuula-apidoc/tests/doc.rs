//! The checked-in reference against the descriptors, and the public
//! export of it.

use kuula_apidoc::{check, render, sections, BEGIN, END};
use kuula_lua::api::{inventory, Group, INTERNAL_MARKER};

fn doc() -> String {
    std::fs::read_to_string(kuula_apidoc::default_path()).expect("docs/api.md")
}

#[test]
fn the_checked_in_reference_is_fresh() {
    if let Err(e) = check(&doc()) {
        panic!("{e}");
    }
}

#[test]
fn rendering_is_deterministic_and_idempotent() {
    let doc = doc();
    let once = render(&doc).unwrap();
    let twice = render(&once).unwrap();
    assert_eq!(once, twice);
    assert_eq!(render(&doc).unwrap(), once);
}

#[test]
fn every_group_has_a_section_with_rows() {
    let inv = inventory();
    let sections = sections(&inv);
    for g in Group::ALL {
        let body = &sections[g.key()];
        assert!(body.lines().count() > 2, "{} has no rows:\n{body}", g.key());
    }
    let doc = doc();
    for g in Group::ALL {
        assert!(
            doc.contains(&format!("{BEGIN}{} -->", g.key())),
            "{} is not in the document",
            g.key()
        );
    }
}

/// The public export drops every line carrying the internal marker
/// from the public copy. The generated sections carry none, so the
/// public copy passes the same check.
#[test]
fn the_public_export_passes_the_check() {
    let doc = doc();
    let public: String = doc
        .lines()
        .filter(|l| !l.contains(INTERNAL_MARKER))
        .map(|l| format!("{l}\n"))
        .collect();
    for (key, body) in sections(&inventory()) {
        assert!(!body.contains(INTERNAL_MARKER), "{key}");
    }
    if let Err(e) = check(&public) {
        panic!("{e}");
    }
}

/// The export drops whole lines, so a marker on a line of code (not a
/// comment) would leave the public tree failing to build. Every
/// source line carrying the marker must be a comment line.
#[test]
fn the_marker_appears_only_on_comment_lines() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let mut offenders = Vec::new();
    let mut checked = 0;
    for top in ["crates", "rom", "examples"] {
        walk(&root.join(top), &mut |path| {
            let comment = match path.extension().and_then(|e| e.to_str()) {
                Some("rs") => "//",
                Some("lua") => "--",
                Some("toml") => "#",
                _ => return,
            };
            let Ok(text) = std::fs::read_to_string(path) else {
                return;
            };
            checked += 1;
            for (n, line) in text.lines().enumerate() {
                if line.contains(INTERNAL_MARKER) && !line.trim_start().starts_with(comment) {
                    offenders.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        });
    }
    assert!(checked > 50, "walked only {checked} files");
    assert!(
        offenders.is_empty(),
        "marker on code lines the export would drop:\n  {}",
        offenders.join("\n  ")
    );
}

fn walk(dir: &std::path::Path, f: &mut dyn FnMut(&std::path::Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            walk(&path, f);
        } else {
            f(&path);
        }
    }
}

#[test]
fn a_stale_section_fails_with_a_diff() {
    let doc = doc();
    let marker = format!("{BEGIN}input -->\n");
    let at = doc.find(&marker).expect("input section") + marker.len();
    let mut stale = doc.clone();
    stale.insert_str(at, "| `btnp(n)` | not there | 1 |\n");
    let err = check(&stale).unwrap_err();
    assert!(err.contains("-| `btnp(n)`"), "{err}");
    assert!(err.contains("--write"), "{err}");
    assert!(stale.contains(END));
}
