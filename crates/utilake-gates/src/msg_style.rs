//! Every error message in the Rust style: a lower-case first letter and no
//! trailing period, so a message reads the same wherever it is embedded,
//! after `: ` in a cause chain or before ` [code]` in a diagnostic.
//!
//! The gate reads the `#[error("…")]` literals and the message argument
//! of `Diagnostic::error`, `Diagnostic::warning` and `Diagnostic::new`
//! (a literal, or the literal of a `format!`) in the shipped source under
//! `crates/` (not tests, benches or anything after a file's first
//! `#[cfg(test)]`, as [`crate::errors_doc`] reads it). A message fails
//! when:
//!
//! - it starts with an upper-case letter, unless its first word is an
//!   acronym or identifier: two or more letters with an upper-case one
//!   after the first, such as `DuckDB`, `HTTP`, `MMS`, `CSV` or `ZIP`;
//! - it ends with `.`.
//!
//! A message starting with `{` (a placeholder) passes the first rule.
//!
//! ## The allowlist
//!
//! `msg_style.allow` at the repository root holds the violations not yet
//! fixed, one per line: `path:line` or the exact message; `#` starts a
//! comment. An entry that no longer matches a violation fails the gate, so
//! the list shrinks as messages are fixed, and [`crate::ratchet`] fails on
//! an entry the base ref's list did not have, so it never grows. With no
//! violation left the file is deleted.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;

use crate::errors_doc::shipped_sources;

/// The allowlist's path under the repository root.
pub const ALLOW: &str = "msg_style.allow";

/// One message that breaks the style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The file, relative to the root.
    pub path: String,
    /// The 1-based line the literal starts on.
    pub line: usize,
    /// The message as written in the source.
    pub message: String,
    /// What is wrong with it.
    pub why: &'static str,
}

/// What is wrong with `message`, if anything.
#[must_use]
pub fn fault(message: &str) -> Option<&'static str> {
    if message.ends_with('.') {
        return Some("ends with a period");
    }
    let first = message.chars().next()?;
    if !first.is_uppercase() {
        return None;
    }
    let word: String = message
        .chars()
        .take_while(|c| c.is_alphanumeric())
        .collect();
    let acronym = word.chars().count() >= 2 && word.chars().skip(1).any(char::is_uppercase);
    (!acronym).then_some("starts with an upper-case letter")
}

/// Every message literal in `text` with the byte offset it starts at.
#[must_use]
pub fn messages(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (at, _) in text.match_indices("#[error(") {
        let rest = &text[at + "#[error(".len()..];
        let skip = rest.len() - rest.trim_start().len();
        if let Some(lit) = literal(&rest[skip..]) {
            out.push((at + "#[error(".len() + skip, lit));
        }
    }
    for (call, nth) in [
        ("Diagnostic::error(", 2),
        ("Diagnostic::warning(", 2),
        ("Diagnostic::new(", 3),
    ] {
        for (at, _) in text.match_indices(call) {
            let start = at + call.len();
            if let Some((offset, arg)) = args(&text[start..]).into_iter().nth(nth) {
                let arg = arg.strip_prefix("format!(").unwrap_or(arg);
                if let Some(lit) = literal(arg) {
                    out.push((start + offset, lit));
                }
            }
        }
    }
    out.sort();
    out
}

/// The string literal `s` starts with, unescaped only as far as `\"` and
/// `\\`; `None` when `s` does not start with one.
fn literal(s: &str) -> Option<String> {
    let mut chars = s.strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => out.push(chars.next()?),
            c => out.push(c),
        }
    }
    None
}

/// The arguments of a call whose `(` was just read, each trimmed, with its
/// byte offset: split at the top-level commas up to the matching `)`.
fn args(s: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let (mut depth, mut start, mut in_str, mut escaped) = (0usize, 0usize, false, false);
    let mut push = |from: usize, to: usize| {
        let arg = &s[from..to];
        let lead = arg.len() - arg.trim_start().len();
        out.push((from + lead, arg.trim()));
    };
    for (i, c) in s.char_indices() {
        if in_str {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => {
                push(start, i);
                break;
            }
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                push(start, i);
                start = i + 1;
            }
            _ => {}
        }
    }
    out
}

/// Every violation in the shipped source under `root`.
#[must_use]
pub fn violations(root: &Path) -> Vec<Violation> {
    let mut out = Vec::new();
    for (path, text) in shipped_sources(root) {
        for (at, message) in messages(&text) {
            if let Some(why) = fault(&message) {
                let line = text[..at].matches('\n').count() + 1;
                out.push(Violation {
                    path: path.clone(),
                    line,
                    message,
                    why,
                });
            }
        }
    }
    out
}

/// The entries of an allowlist's text.
#[must_use]
pub fn entries(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(|l| l.split_once('#').map_or(l, |(e, _)| e).trim())
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The entries of `new` that `old` did not have: the list grew.
#[must_use]
pub fn growth(old: &str, new: &str) -> Vec<String> {
    let old = entries(old);
    entries(new)
        .into_iter()
        .filter(|e| !old.contains(e))
        .collect()
}

/// The violations no entry allows, and the entries that allow none.
#[must_use]
pub fn judge(found: &[Violation], allow: &BTreeSet<String>) -> (Vec<Violation>, Vec<String>) {
    let key = |v: &Violation| format!("{}:{}", v.path, v.line);
    let open = found
        .iter()
        .filter(|v| !allow.contains(&key(v)) && !allow.contains(&v.message))
        .cloned()
        .collect();
    let stale = allow
        .iter()
        .filter(|e| !found.iter().any(|v| key(v) == **e || v.message == **e))
        .cloned()
        .collect();
    (open, stale)
}

/// The gate.
///
/// # Errors
///
/// When the allowlist exists and cannot be read.
pub fn run(root: &Path) -> Result<bool> {
    let found = violations(root);
    let path = root.join(ALLOW);
    let allow = if path.exists() {
        entries(&std::fs::read_to_string(&path)?)
    } else {
        BTreeSet::new()
    };
    let (open, stale) = judge(&found, &allow);
    for v in &open {
        eprintln!("{}:{}: {}: {:?}", v.path, v.line, v.why, v.message);
    }
    for e in &stale {
        eprintln!("{ALLOW}: {e:?} allows nothing any more; remove it");
    }
    if open.is_empty() && stale.is_empty() {
        if found.is_empty() {
            println!("msg-style: every message in the Rust style");
        } else {
            println!(
                "msg-style: every message in the Rust style but the {} {ALLOW} lists",
                found.len()
            );
        }
        return Ok(true);
    }
    if !open.is_empty() {
        eprintln!(
            "msg-style: {} message(s) not in the Rust style: start lower-case (an acronym such as HTTP may lead), no trailing period",
            open.len()
        );
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule() {
        assert_eq!(fault("the file is gone"), None);
        assert_eq!(fault("{path}: {source}"), None);
        assert_eq!(fault("DuckDB refused it"), None);
        assert_eq!(fault("HTTP {status}"), None);
        assert_eq!(fault("MMS"), None);
        assert_eq!(fault("`x` is not a time"), None);
        assert_eq!(fault(""), None);
        assert_eq!(fault("The file"), Some("starts with an upper-case letter"));
        assert_eq!(fault("A D row"), Some("starts with an upper-case letter"));
        assert_eq!(fault("I row"), Some("starts with an upper-case letter"));
        assert_eq!(fault("gone."), Some("ends with a period"));
        assert_eq!(fault("gone..."), Some("ends with a period"));
    }

    #[test]
    fn finds_the_messages() {
        let src = r#"
#[error("The file {path}.")]
#[error(transparent)]
#[error(
    "a \"quoted\" one\\"
)]
let d = Diagnostic::error("csv.x", Category::CSV, "Bad row.");
let d = Diagnostic::warning("csv.y", Category::CSV, format!("Odd ({a}, {b})", a = f(1, 2)));
let d = Diagnostic::new("csv.z", Category::CSV, Severity::Error, msg);
let d = Diagnostic::error("csv.w", [a, b], "fine");
let d = Diagnostic::error("csv.u", c, "say \"hi\", (now)");
let d = Diagnostic::error(code"#;
        let got: Vec<String> = messages(src).into_iter().map(|(_, m)| m).collect();
        assert_eq!(
            got,
            [
                "The file {path}.",
                "a \"quoted\" one\\",
                "Bad row.",
                "Odd ({a}, {b})",
                "fine",
                "say \"hi\", (now)",
            ]
        );
        assert_eq!(literal("\"open"), None);
        assert_eq!(literal("\"esc\\"), None);
        assert_eq!(literal("x"), None);
    }

    #[test]
    fn allowlists_shrink() {
        assert_eq!(
            entries("# c\n\na.rs:3  # why\nThe message.\n"),
            BTreeSet::from(["a.rs:3".to_owned(), "The message.".to_owned()])
        );
        assert_eq!(growth("a\nb\n", "b\nc\n"), ["c"]);
        assert!(growth("a\nb\n", "a\n").is_empty());
        let v = |line, message: &str| Violation {
            path: "a.rs".into(),
            line,
            message: message.into(),
            why: "w",
        };
        let found = [v(3, "One."), v(4, "Two."), v(5, "Three.")];
        let allow = entries("a.rs:3\nTwo.\nGone.\na.rs:9\n");
        let (open, stale) = judge(&found, &allow);
        assert_eq!(open, [v(5, "Three.")]);
        assert_eq!(stale, ["Gone.", "a.rs:9"]);
    }

    #[test]
    fn runs_against_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let src = root.join("crates/a/src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("lib.rs"),
            "#[error(\"fine\")]\n#[error(\"Bad.\")]\n#[cfg(test)]\n#[error(\"Test only.\")]\n",
        )
        .unwrap();
        assert_eq!(
            violations(root),
            [Violation {
                path: "crates/a/src/lib.rs".into(),
                line: 2,
                message: "Bad.".into(),
                why: "ends with a period",
            }]
        );
        assert!(!run(root).unwrap());
        std::fs::write(root.join(ALLOW), "crates/a/src/lib.rs:2\n").unwrap();
        assert!(run(root).unwrap());
        std::fs::write(root.join(ALLOW), "Bad.\nStale.\n").unwrap();
        assert!(!run(root).unwrap());
        std::fs::write(src.join("lib.rs"), "#[error(\"fine\")]\n").unwrap();
        std::fs::remove_file(root.join(ALLOW)).unwrap();
        assert!(run(root).unwrap());
        std::fs::create_dir(root.join(ALLOW)).unwrap();
        assert!(run(root).is_err());
    }
}
