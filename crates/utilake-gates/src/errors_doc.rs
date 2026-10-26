//! `docs/errors.md` lists every diagnostic code. The codes are found in the
//! source as string literals of the form `"category.snake_name"` passed to
//! `Diagnostic::error`/`warning`/`new` or named as a `code` field, so a new
//! failure mode shows up here automatically and the doc cannot drift.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;

/// What a repository's catalogue says about itself.
#[derive(Debug, Clone, Copy)]
pub struct Spec<'a> {
    /// The product named in the heading sentence.
    pub product: &'a str,
    /// Every category, in the order the catalogue lists them.
    pub categories: &'a [&'a str],
    /// Codes that are no longer raised. A code is a promise to whoever
    /// matches on it, so a retired one stays listed and may never be given
    /// a new meaning.
    pub retired: &'a [(&'a str, &'a str)],
    /// Codes raised by a dependency on the repository's behalf, with where
    /// they are raised; listed beside the repository's own.
    pub inherited: &'a [(&'a str, &'a str)],
}

/// The retired codes that live code raises again.
#[must_use]
pub fn reused<'a>(spec: &Spec<'a>, codes: &BTreeMap<String, String>) -> Vec<&'a str> {
    spec.retired
        .iter()
        .map(|(c, _)| *c)
        .filter(|c| codes.contains_key(*c))
        .collect()
}

/// The categories of utilake's own catalogue: the ones every repository
/// shares, then the mirror's.
pub const UTILAKE_CATEGORIES: [&str; 15] = [
    "config", "net", "listing", "cache", "zip", "csv", "json", "parse", "schema", "store",
    "manifest", "verify", "query", "internal", "mirror",
];

/// Every shipped `.rs` file under `root`'s `crates/`, relative path and
/// text, in path order: not `xtask`, `tests` or `benches`, and each cut at
/// its first `#[cfg(test)]`, since test modules invent codes and messages.
#[must_use]
pub fn shipped_sources(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![root.join("crates")];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut entries: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n != "target") {
                    stack.push(p);
                }
                continue;
            }
            if p.extension().is_none_or(|e| e != "rs")
                || p.components().any(|c| {
                    c.as_os_str() == "xtask"
                        || c.as_os_str() == "tests"
                        || c.as_os_str() == "benches"
                })
            {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            let text = text.split("#[cfg(test)]").next().unwrap_or("").to_owned();
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, text));
        }
    }
    out.sort();
    out
}

/// Every `"category.name"` literal in the shipped `.rs` files under
/// `crates/`, with the first file that uses it.
#[must_use]
pub fn collect_codes(root: &Path, categories: &[&str]) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for (rel, text) in shipped_sources(root) {
        for code in literals(&text, categories) {
            out.entry(code).or_insert_with(|| rel.clone());
        }
    }
    out
}

/// The codes passed to `Diagnostic::error`, `warning` or `new` in the
/// shipped source whose category is not one of `categories`, with the
/// file: a code the catalogue would silently leave out.
#[must_use]
pub fn undeclared(root: &Path, categories: &[&str]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (rel, text) in shipped_sources(root) {
        for call in [
            "Diagnostic::error(",
            "Diagnostic::warning(",
            "Diagnostic::new(",
        ] {
            for (at, _) in text.match_indices(call) {
                let rest = text[at + call.len()..].trim_start();
                let Some(lit) = rest
                    .strip_prefix('"')
                    .and_then(|r| r.split_once('"'))
                    .map(|(l, _)| l)
                else {
                    continue;
                };
                if lit
                    .split_once('.')
                    .is_none_or(|(cat, _)| !categories.contains(&cat))
                {
                    out.push((lit.to_owned(), rel.clone()));
                }
            }
        }
    }
    out
}

/// The code literals in `text`.
#[must_use]
pub fn literals(text: &str, categories: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(q) = text[i..].find('"') {
        let start = i + q + 1;
        let Some(len) = text[start..].find('"') else {
            break;
        };
        let lit = &text[start..start + len];
        i = start + len + 1;
        if let Some((cat, name)) = lit.split_once('.')
            && categories.contains(&cat)
            && !name.is_empty()
            && !matches!(
                name,
                "toml"
                    | "json"
                    | "rs"
                    | "md"
                    | "yaml"
                    | "csv"
                    | "zip"
                    | "sqlite"
                    | "html"
                    | "txt"
                    | "parquet"
            )
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'_' || b.is_ascii_digit())
            && !name.contains("..")
        {
            // Skip prose like "the file.name" by requiring the literal to stand alone.
            let before = bytes[..start - 1]
                .iter()
                .rev()
                .find(|b| !b.is_ascii_whitespace())
                .copied();
            if matches!(before, Some(b'(' | b',' | b'=' | b'[' | b'{' | b'>' | b':')) {
                out.push(lit.to_owned());
            }
        }
    }
    out
}

/// Renders the catalogue.
#[must_use]
pub fn render(spec: &Spec<'_>, codes: &BTreeMap<String, String>) -> String {
    use std::fmt::Write as _;
    let mut s = format!(
        "# Diagnostic codes\n\nEvery error and warning {} reports carries one of these codes. Codes are stable: never renamed, never reused. The category is the first segment. This file is generated by `just errors-doc`; `just errors-check` fails when it is out of date.\n\n",
        spec.product
    );
    let mut by_cat: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for (code, file) in codes {
        let cat = code.split_once('.').map_or("", |(c, _)| c);
        by_cat.entry(cat).or_default().push((code, file));
    }
    for cat in spec.categories {
        let Some(list) = by_cat.get(cat) else {
            continue;
        };
        let _ = write!(s, "## {cat}\n\n| code | raised in |\n|---|---|\n");
        for (code, file) in list {
            let _ = writeln!(s, "| `{code}` | `{file}` |");
        }
        s.push('\n');
    }
    s.push_str("## Retired\n\nNo longer raised; kept so the code is never reused.\n\n| code | was |\n|---|---|\n");
    for (code, was) in spec.retired {
        let _ = writeln!(s, "| `{code}` | {was} |");
    }
    // Exactly one trailing newline, as the end-of-file hook enforces.
    s.trim_end().to_owned() + "\n"
}

/// The gate: regenerates `docs/errors.md` under `root` with `write`, or
/// fails with `check` when it would change.
pub fn run(root: &Path, spec: &Spec<'_>, write: bool, check: bool) -> Result<bool> {
    let stray = undeclared(root, spec.categories);
    if !stray.is_empty() {
        for (code, file) in &stray {
            eprintln!("{file}: diagnostic code {code:?} is under no declared category");
        }
        return Ok(false);
    }
    let mut codes = collect_codes(root, spec.categories);
    for (code, from) in spec.inherited {
        codes
            .entry((*code).to_owned())
            .or_insert_with(|| (*from).to_owned());
    }
    let again = reused(spec, &codes);
    if !again.is_empty() {
        eprintln!(
            "retired diagnostic code(s) raised again: {}; pick a new code",
            again.join(", ")
        );
        return Ok(false);
    }
    let text = render(spec, &codes);
    let path = root.join("docs/errors.md");
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    if write {
        std::fs::create_dir_all(path.parent().expect("docs/"))?;
        std::fs::write(&path, &text)?;
        println!("docs/errors.md: {} codes written", codes.len());
        return Ok(true);
    }
    if check && current != text {
        eprintln!("docs/errors.md is out of date; run `just errors-doc`");
        return Ok(false);
    }
    println!("docs/errors.md: {} codes, up to date", codes.len());
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATS: [&str; 3] = ["csv", "net", "schema"];
    const SPEC: Spec<'static> = Spec {
        product: "the tool",
        categories: &CATS,
        retired: &[("schema.old", "gone")],
        inherited: &[("net.inherited", "a dependency")],
    };

    #[test]
    fn finds_codes() {
        let src = r#"Diagnostic::error("csv.field_count_mismatch", Category::Csv, "x"); let c = "net.offline"; code: "schema.drift", "not a code", "the file.txt", "config.toml", "csv.zip", "net.Mixed", "unterminated"#;
        let l = literals(src, &CATS);
        assert_eq!(
            l,
            ["csv.field_count_mismatch", "net.offline", "schema.drift"]
        );
        assert!(literals("no quotes", &CATS).is_empty());
        let mut m = BTreeMap::new();
        m.insert("net.offline".to_owned(), "crates/x.rs".to_owned());
        let r = render(&SPEC, &m);
        assert!(r.contains("the tool reports"));
        assert!(r.contains("## net") && r.contains("`net.offline`"));
        assert!(r.contains("## Retired") && r.contains("`schema.old`"));
        assert!(reused(&SPEC, &m).is_empty());
        m.insert("schema.old".to_owned(), "crates/y.rs".to_owned());
        assert_eq!(reused(&SPEC, &m), ["schema.old"]);
    }

    #[test]
    fn runs_against_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let src = root.join("crates/a/src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(root.join("crates/a/tests")).unwrap();
        std::fs::write(
            src.join("lib.rs"),
            "let d = Diagnostic::error(\"csv.bad\", c, m);\n#[cfg(test)]\nmod t { let x = \"csv.test_only\"; }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates/a/tests/t.rs"),
            "let x = \"csv.in_tests\";",
        )
        .unwrap();
        assert!(!run(root, &SPEC, false, true).unwrap());
        assert!(run(root, &SPEC, true, false).unwrap());
        let doc = std::fs::read_to_string(root.join("docs/errors.md")).unwrap();
        assert!(doc.contains("`csv.bad`") && doc.contains("`crates/a/src/lib.rs`"));
        assert!(doc.contains("`net.inherited`") && doc.contains("`a dependency`"));
        assert!(!doc.contains("csv.test_only") && !doc.contains("csv.in_tests"));
        assert!(run(root, &SPEC, false, true).unwrap());
        assert!(run(root, &SPEC, false, false).unwrap());
        std::fs::write(src.join("more.rs"), "code: \"schema.old\",").unwrap();
        assert!(!run(root, &SPEC, false, true).unwrap());
        // A source that is not text is passed over; a root without crates
        // has no codes.
        std::fs::write(src.join("bytes.rs"), [0xffu8, 0xfe, b'"', b'c']).unwrap();
        std::fs::remove_file(src.join("more.rs")).unwrap();
        assert!(run(root, &SPEC, false, true).unwrap());
        let bare = tempfile::tempdir().unwrap();
        assert!(collect_codes(bare.path(), &CATS).is_empty());
        // A code under a category the spec does not declare fails the gate.
        std::fs::write(
            src.join("stray.rs"),
            "Diagnostic::warning(\"mirror.crawl\", c, m); Diagnostic::new(code, c, s, m); Diagnostic::error(\"nodot\", c, m);",
        )
        .unwrap();
        assert_eq!(
            undeclared(root, &CATS),
            [
                ("nodot".to_owned(), "crates/a/src/stray.rs".to_owned()),
                (
                    "mirror.crawl".to_owned(),
                    "crates/a/src/stray.rs".to_owned()
                )
            ]
        );
        assert!(!run(root, &SPEC, false, true).unwrap());
        assert!(UTILAKE_CATEGORIES.contains(&"mirror"));
    }
}
