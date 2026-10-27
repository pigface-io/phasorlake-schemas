//! AEMO's data model as its report pages declare it, for a market whose
//! model ships as a web report and not as a create script: the Gas Data
//! Model Report under `Reports/CURRENT/MMSDataModelReport/Gas/`.
//!
//! PowerDesigner writes the report as a frame page, a table of contents
//! (`{Report}_toc.htm`) and one page per package and table (`Gas_N.htm`).
//! The contents name each `Package: NAME` and `Table: NAME` with its page,
//! and the model's version (`Description of the model AEMO Gas Data Model
//! v2.2.`). A table's page carries a form with its name and comment, a
//! grid of its primary key columns, and a grid of its columns: name, data
//! type as written (`NUMBER(18,3)`, `VARCHAR2(20)`, `DATE`), whether
//! mandatory, and comment. Read into the same [`Model`] a create script
//! is, so the fold and the export treat both alike; the report's identity
//! is the SHA-256 of its table of contents.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use phasor_iceberg::Market;

use crate::ctl::sha256;
use crate::models::{Model, ModelError, ModelTable, Recorded, record_fresh};
use crate::month::YearMonth;

/// What the table of contents says: the version, the model's name, and
/// each table's page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contents {
    /// `2.2`, when the contents name it.
    pub version: Option<String>,
    /// `AEMO Gas Data Model v2.2`.
    pub model: String,
    /// Table name to the page that describes it.
    pub pages: BTreeMap<String, String>,
}

/// The text of an HTML fragment: tags out, entities decoded, whitespace
/// as single spaces, trimmed.
fn text_of(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        match rest[lt..].find('>') {
            Some(gt) => rest = &rest[lt + gt + 1..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every `<tag …>…</tag>` block of `html`, inner HTML, in order; a block is
/// taken to the first closing tag, which the report never nests.
fn blocks<'a>(html: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find(&open) {
        let after = &rest[at + open.len()..];
        // `<td` must not match `<tdx`: the tag ends in a space or `>`.
        if !after.starts_with([' ', '>', '\n']) {
            rest = after;
            continue;
        }
        let Some(gt) = after.find('>') else { break };
        let inner_start = &after[gt + 1..];
        let Some(end) = inner_start.find(&close) else {
            break;
        };
        out.push(&inner_start[..end]);
        rest = &inner_start[end + close.len()..];
    }
    out
}

/// The anchors of the table of contents, `(href, text)`, in order.
fn anchors(html: &str) -> Vec<(String, String)> {
    blocks(html, "a")
        .into_iter()
        .zip(html.match_indices("<a ").map(|(i, _)| &html[i..]))
        .filter_map(|(inner, open)| {
            let href = open.split_once("href=\"")?.1.split_once('"')?.0;
            Some((href.to_owned(), text_of(inner)))
        })
        .collect()
}

/// Reads the table of contents.
pub fn contents(toc: &str) -> Contents {
    let mut out = Contents {
        version: None,
        model: String::new(),
        pages: BTreeMap::new(),
    };
    for (href, text) in anchors(toc) {
        if let Some(name) = text.strip_prefix("Table: ") {
            let page = href.split_once('#').map_or(href.as_str(), |(p, _)| p);
            out.pages.insert(name.to_owned(), page.to_owned());
        } else if let Some(model) = text.strip_prefix("Description of the model ") {
            let model = model.trim_end_matches('.');
            out.model = model.to_owned();
            out.version = model
                .rsplit(' ')
                .next()
                .and_then(|w| w.strip_prefix('v'))
                .filter(|v| v.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
                .map(str::to_owned);
        }
    }
    out
}

/// One table's page, read: its name, and the table.
pub fn table_page(html: &str) -> Option<(String, ModelTable)> {
    let name = blocks(html, "h2")
        .into_iter()
        .map(text_of)
        .find_map(|t| t.strip_prefix("Table: ").map(str::to_owned))?;
    let mut table = ModelTable {
        comment: String::new(),
        primary_key: Vec::new(),
        columns: Vec::new(),
    };
    // Rows in order, each heading before them saying what they are.
    let mut section = String::new();
    let mut at = 0;
    let headings: Vec<(usize, String)> = html
        .match_indices("<h3")
        .map(|(i, _)| {
            (
                i,
                blocks(&html[i..], "h3")
                    .first()
                    .map(|b| text_of(b))
                    .unwrap_or_default(),
            )
        })
        .collect();
    for (i, _) in html.match_indices("<tr") {
        while let Some((h, title)) = headings.get(at)
            && *h < i
        {
            section = title.clone();
            at += 1;
        }
        let row = blocks(&html[i..], "tr")
            .first()
            .copied()
            .unwrap_or_default();
        let cells: Vec<String> = blocks(row, "td").into_iter().map(text_of).collect();
        match (section.as_str(), cells.as_slice()) {
            (_, [k, v]) if k == "Comment" => table.comment = v.clone(),
            ("Primary Key Columns", [c]) if c != "Name" => table.primary_key.push(c.clone()),
            ("Content", [n, ty, mandatory, comment]) if n != "Name" => {
                table
                    .columns
                    .push((n.clone(), ty.clone(), mandatory != "X", comment.clone()));
            }
            _ => {}
        }
    }
    Some((name, table))
}

/// The report's pages directory: `dir` when it holds the contents, else
/// the one `*_files` directory under it that does.
fn pages_dir(dir: &Path) -> Result<(PathBuf, PathBuf), ModelError> {
    let toc_in = |d: &Path| -> Option<PathBuf> {
        std::fs::read_dir(d)
            .ok()?
            .filter_map(Result::ok)
            .find_map(|e| {
                let p = e.path();
                let name = p.file_name()?.to_str()?;
                (name.ends_with("_toc.htm") && !name.starts_with("._")).then_some(p)
            })
    };
    if let Some(toc) = toc_in(dir) {
        return Ok((dir.to_path_buf(), toc));
    }
    let under = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.to_string_lossy().ends_with("_files"))
        .find_map(|d| toc_in(&d).map(|t| (d, t)));
    under.ok_or_else(|| ModelError::Read {
        path: dir.display().to_string(),
        message:
            "no `*_toc.htm` of a data model report is in it or in a `*_files` directory under it"
                .into(),
    })
}

/// The report under `dir` as a model first and last seen in `month`:
/// `version` when its contents do not say. Tables whose pages cannot be
/// read are left out and named in `problems`.
pub fn read_report(
    dir: &Path,
    month: YearMonth,
    version: Option<&str>,
) -> Result<(Model, Vec<String>), ModelError> {
    let (pages, toc) = pages_dir(dir)?;
    let read = |p: &Path| {
        std::fs::read(p).map_err(|e| ModelError::Read {
            path: p.display().to_string(),
            message: e.to_string(),
        })
    };
    let toc_bytes = read(&toc)?;
    let c = contents(&String::from_utf8_lossy(&toc_bytes));
    let version = version
        .map(str::to_owned)
        .or(c.version)
        .ok_or_else(|| ModelError::Read {
            path: toc.display().to_string(),
            message: "the contents name no version; give one with --version".into(),
        })?;
    let mut problems = Vec::new();
    let mut tables = BTreeMap::new();
    for (name, page) in &c.pages {
        let path = pages.join(page);
        let html = match read(&path) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) => {
                problems.push(e.to_string());
                continue;
            }
        };
        match table_page(&html) {
            Some((found, t)) if found == *name => {
                tables.insert(found, t);
            }
            _ => problems.push(format!(
                "The page {} does not describe table {name}.",
                path.display()
            )),
        }
    }
    Ok((
        Model {
            version,
            model: c.model,
            created: String::new(),
            script_sha256: sha256(&toc_bytes),
            first_seen: month,
            last_seen: month,
            tables,
        },
        problems,
    ))
}

/// Records the report under `dir` as `market`'s model of `month` (see
/// [`crate::models::record`]).
pub fn record(
    root: &Path,
    market: Market,
    month: YearMonth,
    dir: &Path,
    version: Option<&str>,
) -> Result<Recorded, ModelError> {
    let (model, problems) = read_report(dir, month, version)?;
    let mut recorded = Recorded {
        problems,
        ..Recorded::default()
    };
    record_fresh(
        root,
        market,
        month,
        vec![(model, dir.display().to_string())],
        &mut recorded,
    )?;
    Ok(recorded)
}

#[cfg(test)]
mod tests;
