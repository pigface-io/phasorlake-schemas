use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use super::*;
use crate::fold::history_path;

const SCADA: &str = r#"name = "DISPATCH_UNIT_SCADA"
package = "DISPATCH"
report_pairs = [["DISPATCH", "UNIT_SCADA"]]
primary_key = ["SETTLEMENTDATE", "DUID"]
time_column = "SETTLEMENTDATE"
load_mode = "append"
visibility = "PUBLIC"

[temporal]
origin = "derived"
entity = ["DUID"]
valid = { kind = "interval_end", column = "SETTLEMENTDATE" }
order = ["LASTCHANGED"]
cutoff = "LASTCHANGED"
"#;

/// Release 5.6 has the table and SCADAVALUE; 5.7, shipped later, has the
/// table without SCADAVALUE.
const MODEL_56: &str = "version = \"5.6\"\nmodel = \"m\"\ncreated = \"c\"\nscript_sha256 = \"s\"\nfirst_seen = \"2025-01\"\nlast_seen = \"2026-04\"\n\n[tables.DISPATCH_UNIT_SCADA]\ncomment = \"Old words\"\nprimary_key = [\"SETTLEMENTDATE\", \"DUID\"]\ncolumns = [\n    [\"SETTLEMENTDATE\", \"DATE\", false, \"Old\"],\n    [\"scadavalue\", \"NUMERIC(16,6)\", true, \"MW\"],\n]\n";
const MODEL_57: &str = "version = \"5.7\"\nmodel = \"m\"\ncreated = \"c\"\nscript_sha256 = \"t\"\nfirst_seen = \"2026-05\"\nlast_seen = \"2026-09\"\n\n[tables.DISPATCH_UNIT_SCADA]\ncomment = \"Unit SCADA\"\nprimary_key = [\"SETTLEMENTDATE\", \"DUID\"]\ncolumns = [\n    [\"SETTLEMENTDATE\", \"TIMESTAMP(0)\", false, \"Interval end\"],\n    [\"DUID\", \"VARCHAR(20)\", false, \"\"],\n]\n";

fn mark(file: &str, t: Option<&str>) -> serde_json::Value {
    json!({"file": file, "sha256": "ab", "release": "2026-08", "data-time": t})
}

fn field(id: u32, name: &str, ty: &str, required: bool) -> serde_json::Value {
    json!({"id": id, "name": name, "type": ty, "required": required})
}

/// The history of DISPATCH_UNIT_SCADA: two schemas, the second moving
/// SCADAVALUE before DUID, one run each.
pub(crate) fn sample() -> History {
    let t = field(1, "SETTLEMENTDATE", "timestamptz", true);
    let duid = field(2, "DUID", "string", true);
    let scada = field(3, "SCADAVALUE", "decimal(16, 6)", false);
    serde_json::from_value(json!({
        "table": "DISPATCH_UNIT_SCADA",
        "format-version": 3,
        "current-schema-id": 1,
        "schemas": [
            {"type": "struct", "schema-id": 0, "identifier-field-ids": [1, 2],
             "fields": [t, duid, scada]},
            {"type": "struct", "schema-id": 1, "identifier-field-ids": [1, 2],
             "fields": [t, scada, duid]},
        ],
        "runs": [
            {"schema-id": 0, "report": ["DISPATCH", "UNIT_SCADA"], "version": 1, "kinds": ["archive"],
             "from": mark("A.zip", Some("2026-08-01T00:05:00+10:00")), "to": mark("B.zip", None),
             "files": 2, "rows": 9, "changes": ["first"], "updates": [], "overlaps": false},
            {"schema-id": 1, "report": ["DISPATCH", "UNIT_SCADA"], "version": 2, "kinds": ["archive"],
             "from": mark("C.zip", None), "to": mark("C.zip", None), "files": 1, "rows": 3,
             "changes": ["schema", "version"],
             "updates": [{"action": "move", "field-id": 3},
                         {"action": "set-identifier-fields", "field-ids": [1, 2]}],
             "overlaps": false},
        ],
        "breaking": [],
    }))
    .unwrap()
}

/// A repository with the declaration, two releases and the history,
/// exported.
pub(crate) fn exported() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let put = |rel: &str, text: &str| {
        crate::files::write_atomic(&root.join(rel), text.as_bytes()).unwrap();
    };
    put("tables/DISPATCH_UNIT_SCADA.toml", SCADA);
    put("evidence/models/5.6.toml", MODEL_56);
    put("evidence/models/5.7.toml", MODEL_57);
    put(
        "evidence/histories/DISPATCH_UNIT_SCADA.json",
        &crate::fold::render(&sample()),
    );
    export(root);
    dir
}

fn export(root: &Path) -> Rendered {
    let r = Export::load(root, DEFAULT_BASE_URL).unwrap().render();
    write(root, &r).unwrap();
    r
}

/// The history grown as a later fold grows it: more files at the end of
/// the last run, SCADAVALUE turned to text in a new field and schema, and
/// a run of it; exported.
pub(crate) fn grow(root: &Path) {
    let mut h = sample();
    h.runs[1].files += 4;
    let mut v = serde_json::to_value(&h).unwrap();
    let s2 = json!({"type": "struct", "schema-id": 2, "identifier-field-ids": [1, 2], "fields": [
        field(1, "SETTLEMENTDATE", "timestamptz", true),
        field(2, "DUID", "string", true),
        field(4, "SCADAVALUE", "string", false),
    ]});
    v["schemas"].as_array_mut().unwrap().push(s2);
    let mut run = v["runs"][1].clone();
    run["schema-id"] = json!(2);
    run["from"] = mark("D.zip", None);
    run["to"] = mark("D.zip", None);
    run["changes"] = json!(["schema"]);
    run["updates"] = json!([{"action": "delete", "field-id": 3}, {"action": "add", "field-id": 4}]);
    v["runs"].as_array_mut().unwrap().push(run);
    v["current-schema-id"] = json!(2);
    v["breaking"] = json!([{"column": "SCADAVALUE", "old-field-id": 3, "old-type": "decimal(16, 6)",
        "new-field-id": 4, "new-type": "string", "at": mark("D.zip", None)}]);
    let h: History = serde_json::from_value(v).unwrap();
    crate::files::write_atomic(
        &history_path(root, &h.table),
        crate::fold::render(&h).as_bytes(),
    )
    .unwrap();
    export(root);
}

fn read(root: &Path, rel: &str) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(root.join(rel)).unwrap()).unwrap()
}

#[test]
fn table_json_describes_the_table_and_holds_its_history() {
    let dir = exported();
    let root = dir.path();
    let t = read(root, "nem/DISPATCH_UNIT_SCADA/table.json");
    let keys: Vec<&String> = t.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "columns",
            "declared",
            "description",
            "iceberg",
            "load_mode",
            "name",
            "package",
            "primary_key",
            "report_pairs",
            "temporal",
            "time_column",
            "visibility",
            "x-format"
        ]
    );
    assert_eq!(t["x-format"], 2);
    assert_eq!(t["declared"], true);
    assert_eq!(
        t["description"], "Unit SCADA",
        "the latest release with the table"
    );
    assert_eq!(t["package"], "DISPATCH");
    assert_eq!(t["primary_key"], json!(["SETTLEMENTDATE", "DUID"]));
    assert_eq!(t["report_pairs"], json!([["DISPATCH", "UNIT_SCADA"]]));
    assert_eq!(t["time_column"], "SETTLEMENTDATE");
    assert_eq!(t["load_mode"], "append");
    assert_eq!(t["visibility"], "PUBLIC");
    assert_eq!(t["temporal"]["valid"]["kind"], "interval_end");
    assert_eq!(
        t["columns"],
        json!([
            {"id": 1, "name": "SETTLEMENTDATE", "iceberg_type": "timestamptz", "comment": "Interval end",
             "model_type": "TIMESTAMP(0)", "first_release": "2026-08", "last_release": "2026-08"},
            {"id": 2, "name": "DUID", "iceberg_type": "string", "comment": null, "model_type": "VARCHAR(20)",
             "first_release": "2026-08", "last_release": "2026-08"},
            // Only 5.6 has it: the latest release with the column.
            {"id": 3, "name": "SCADAVALUE", "iceberg_type": "decimal(16, 6)", "comment": "MW",
             "model_type": "NUMERIC(16,6)", "first_release": "2026-08", "last_release": "2026-08"},
        ])
    );
    // The table as Iceberg metadata, at its published location.
    let m = read(root, "nem/DISPATCH_UNIT_SCADA/metadata/v1.metadata.json");
    assert_eq!(m["format-version"], 3);
    assert_eq!(m["current-schema-id"], 1);
    assert_eq!(m["schemas"].as_array().unwrap().len(), 2);
    assert_eq!(
        m["location"],
        format!("{DEFAULT_BASE_URL}nem/DISPATCH_UNIT_SCADA")
    );
    assert_eq!(
        std::fs::read_to_string(root.join("nem/DISPATCH_UNIT_SCADA/metadata/version-hint.text"))
            .unwrap(),
        "1"
    );
    // Every header, with its table and schemas.
    let hs = read(root, "nem/headers.json");
    assert_eq!(hs["x-format"], 2);
    assert_eq!(
        hs["headers"],
        json!([
            {"report": ["DISPATCH", "UNIT_SCADA"], "version": 1,
             "columns": ["SETTLEMENTDATE", "DUID", "SCADAVALUE"], "table": "DISPATCH_UNIT_SCADA",
             "schemas": [{"schema-id": 0, "first-release": "2026-08", "last-release": "2026-08", "runs": 1}]},
            {"report": ["DISPATCH", "UNIT_SCADA"], "version": 2,
             "columns": ["SETTLEMENTDATE", "SCADAVALUE", "DUID"], "table": "DISPATCH_UNIT_SCADA",
             "schemas": [{"schema-id": 1, "first-release": "2026-08", "last-release": "2026-08", "runs": 1}]},
        ])
    );
    // Each header resolves as the history resolves it.
    let h = sample();
    for e in hs["headers"].as_array().unwrap() {
        let columns: Vec<&str> = e["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        let report = [
            e["report"][0].as_str().unwrap(),
            e["report"][1].as_str().unwrap(),
        ];
        let version = u16::try_from(e["version"].as_u64().unwrap()).unwrap();
        let first = &e["schemas"][0];
        assert_eq!(
            h.resolve(
                report,
                version,
                &columns,
                first["first-release"].as_str().unwrap()
            ),
            first["schema-id"]
                .as_u64()
                .map(|i| u32::try_from(i).unwrap())
        );
    }
    // The history as the fold wrote it, each schema's document named.
    let mut iceberg = t["iceberg"].clone();
    let files = iceberg
        .as_object_mut()
        .unwrap()
        .remove("schema-files")
        .unwrap();
    assert_eq!(iceberg, serde_json::to_value(&h).unwrap());
    for s in &h.schemas {
        let name = schema::file_name(s);
        assert_eq!(files[s.schema_id.to_string()], json!(name));
        assert!(root.join("nem/DISPATCH_UNIT_SCADA").join(&name).is_file());
    }
    // The fold reads the history back out of it.
    let published = crate::fold::published(root).unwrap();
    assert_eq!(published["DISPATCH_UNIT_SCADA"], h);
}

/// A header seen in several runs of one schema counts them and reaches
/// their last release; a run whose schema the history lacks names no
/// header.
#[test]
fn headers_count_their_runs_and_reach_their_last_release() {
    let mut h = sample();
    let mut again = h.runs[0].clone();
    again.from.release = "2026-09".into();
    again.to.release = "2026-10".into();
    h.runs.push(again);
    let mut lost = h.runs[1].clone();
    lost.schema_id = 9;
    h.runs.push(lost);
    let hs = headers([&h].into_iter());
    let first = &hs["headers"][0]["schemas"][0];
    assert_eq!(
        (
            first["runs"].clone(),
            first["first-release"].clone(),
            first["last-release"].clone()
        ),
        (json!(2), json!("2026-08"), json!("2026-10"))
    );
    assert_eq!(hs["headers"].as_array().unwrap().len(), 2);
}

#[test]
fn a_schema_document_is_named_by_its_fields_and_never_changes() {
    let dir = exported();
    let root = dir.path();
    let h = sample();
    let name = schema::file_name(&h.schemas[0]);
    assert_eq!(name, "schema-2e75294ff617bace.json");
    // The id is not part of the name, the order is.
    let mut renumbered = h.schemas[0].clone();
    renumbered.schema_id = 7;
    assert_eq!(schema::file_name(&renumbered), name);
    assert_ne!(schema::file_name(&h.schemas[1]), name);
    let rel = format!("nem/DISPATCH_UNIT_SCADA/{name}");
    let before = std::fs::read(root.join(&rel)).unwrap();
    let d: serde_json::Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(d["$id"], format!("{DEFAULT_BASE_URL}{rel}"));
    assert_eq!(d["$schema"], "https://json-schema.org/draft/2020-12/schema");
    assert_eq!(d["x-format"], 2);
    assert_eq!(
        d["x-table"],
        json!({"name": "DISPATCH_UNIT_SCADA", "$ref": "table.json"})
    );
    assert_eq!(d["x-schema-id"], 0);
    assert_eq!(d["x-identifier-field-ids"], json!([1, 2]));
    assert_eq!(d["x-fields"][2], json!([3, "SCADAVALUE", "decimal(16, 6)"]));
    assert_eq!(d["required"], json!(["SETTLEMENTDATE", "DUID"]));
    assert_eq!(d["additionalProperties"], false);
    let p = &d["properties"];
    assert_eq!(
        p["SETTLEMENTDATE"],
        json!({"type": "string", "format": "date-time", "x-field-id": 1, "x-iceberg-type": "timestamptz"})
    );
    assert_eq!(p["SCADAVALUE"]["type"], json!(["number", "null"]));
    assert!(d.get("x-runs").is_none());
    grow(root);
    assert_eq!(
        std::fs::read(root.join(&rel)).unwrap(),
        before,
        "more runs, the same bytes"
    );
    let types: Vec<serde_json::Value> = ["boolean", "long", "double", "date", "string"]
        .iter()
        .map(|t| {
            let s = one_column(t);
            let doc: serde_json::Value =
                serde_json::from_str(&schema::document("b/", "T", "p", &s)).unwrap();
            doc["properties"]["C"].clone()
        })
        .collect();
    assert_eq!(
        types,
        [
            json!({"type": "boolean", "x-field-id": 1, "x-iceberg-type": "boolean"}),
            json!({"type": ["integer", "null"], "x-field-id": 1, "x-iceberg-type": "long"}),
            json!({"type": ["number", "null"], "x-field-id": 1, "x-iceberg-type": "double"}),
            json!({"type": ["string", "null"], "format": "date", "x-field-id": 1, "x-iceberg-type": "date"}),
            json!({"type": ["string", "null"], "x-field-id": 1, "x-iceberg-type": "string"}),
        ]
    );
}

/// A schema of one column `C` of type `ty`, required when boolean.
fn one_column(ty: &str) -> utilake_iceberg::Schema {
    utilake_iceberg::Schema::from_json(
        &json!({"schema-id": 0, "fields": [field(1, "C", ty, ty == "boolean")]}),
    )
    .unwrap()
}

#[test]
fn the_manifest_names_every_file_and_the_export_owns_nem() {
    let dir = exported();
    let root = dir.path();
    let m = read(root, "manifest.json");
    assert_eq!(m["format"], 2);
    assert_eq!(m["market"], "nem");
    assert_eq!(m["base_url"], DEFAULT_BASE_URL);
    assert_eq!(m["baseline"], json!({"version": 1, "reason": ""}));
    assert_eq!(
        (m["tables"].clone(), m["schemas"].clone()),
        (json!(1), json!(2))
    );
    // A re-based root says so.
    std::fs::write(
        crate::fold::baseline_path(root),
        "version = 2\nreason = \"why\"\n",
    )
    .unwrap();
    let r = Export::load(root, DEFAULT_BASE_URL).unwrap().render();
    let m: serde_json::Value = serde_json::from_str(&r.files["manifest.json"]).unwrap();
    assert_eq!(m["baseline"], json!({"version": 2, "reason": "why"}));
    std::fs::write(crate::fold::baseline_path(root), "version = ").unwrap();
    assert!(Export::load(root, DEFAULT_BASE_URL).is_err());
    std::fs::remove_file(crate::fold::baseline_path(root)).unwrap();
    assert_eq!(
        m["listed"],
        json!([{"name": "DISPATCH_UNIT_SCADA", "package": "DISPATCH", "declared": true,
                "first_release": "2026-08", "last_release": "2026-08",
                "schemas": 2, "runs": 2, "breaks": 0}])
    );
    let listed = m["sha256"].as_object().unwrap();
    assert_eq!(listed.len(), 6);
    for (p, sha) in listed {
        assert_eq!(
            crate::ctl::sha256(&std::fs::read(root.join(p)).unwrap()),
            *sha
        );
    }
    assert!(!root.join("provenance.json").exists());

    // Again: the same bytes, nothing written.
    let r = Export::load(root, DEFAULT_BASE_URL).unwrap().render();
    assert_eq!(r, Export::load(root, DEFAULT_BASE_URL).unwrap().render());
    assert!(check(root, &r).unwrap().is_empty());
    assert_eq!(
        write(root, &r).unwrap(),
        Written {
            written: 0,
            kept: 7,
            removed: 0
        }
    );

    // Stale: a file of nobody's, one edited, one gone.
    std::fs::create_dir_all(root.join("nem/OLD/deep")).unwrap();
    std::fs::write(root.join("nem/OLD/deep/x.json"), "{}").unwrap();
    std::fs::write(root.join("nem/DISPATCH_UNIT_SCADA/old.json"), "{}").unwrap();
    std::fs::write(root.join("manifest.json"), "{}").unwrap();
    let table = root.join("nem/DISPATCH_UNIT_SCADA/table.json");
    std::fs::remove_file(&table).unwrap();
    let stale: Vec<String> = check(root, &r)
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        stale,
        [
            "manifest.json differs",
            "nem/DISPATCH_UNIT_SCADA/old.json extra",
            "nem/DISPATCH_UNIT_SCADA/table.json missing",
            "nem/OLD/deep/x.json extra"
        ]
    );
    assert_eq!(
        write(root, &r).unwrap(),
        Written {
            written: 2,
            kept: 5,
            removed: 2
        }
    );
    assert!(!root.join("nem/OLD").exists(), "emptied directories go");
    assert!(root.join("nem").is_dir());
    assert!(check(root, &r).unwrap().is_empty());

    // A table without a declaration or a release.
    let mut other = sample();
    other.table = "OTHER".into();
    crate::files::write_atomic(
        &history_path(root, "OTHER"),
        crate::fold::render(&other).as_bytes(),
    )
    .unwrap();
    std::fs::write(
        crate::fold::histories_dir(root).join("notes.txt"),
        "not a history",
    )
    .unwrap();
    let r = export(root);
    assert_eq!((r.tables, r.schemas), (2, 4));
    let t = read(root, "nem/OTHER/table.json");
    assert_eq!(t["declared"], false);
    for k in [
        "description",
        "primary_key",
        "report_pairs",
        "time_column",
        "temporal",
        "load_mode",
        "visibility",
    ] {
        assert_eq!(t[k], json!(null), "{k}");
    }
    assert_eq!(
        t["columns"][0],
        json!({"id": 1, "name": "SETTLEMENTDATE", "iceberg_type": "timestamptz", "comment": null,
               "model_type": null, "first_release": "2026-08", "last_release": "2026-08"})
    );
    assert_eq!(import_check(root), Ok(2));
}

#[test]
fn what_cannot_be_read_or_written_is_named() {
    let dir = exported();
    let root = dir.path();
    let r = Export::load(root, DEFAULT_BASE_URL).unwrap().render();
    let e = |x: Result<Written, ExportError>| x.unwrap_err().to_string();
    // A directory where a file is to be.
    let table = root.join("nem/DISPATCH_UNIT_SCADA/table.json");
    std::fs::remove_file(&table).unwrap();
    std::fs::create_dir(&table).unwrap();
    assert!(
        check(root, &r)
            .unwrap_err()
            .to_string()
            .ends_with("table.json.")
    );
    assert!(e(write(root, &r)).ends_with("table.json."));
    std::fs::remove_dir(&table).unwrap();
    // A file where nem/ is.
    let flat = tempfile::tempdir().unwrap();
    std::fs::write(flat.path().join("nem"), "x").unwrap();
    assert!(check(flat.path(), &Rendered::default()).is_err());
    assert!(e(write(flat.path(), &Rendered::default())).contains("nem"));

    // Histories: misnamed, not a history, not a directory.
    let hist = crate::fold::histories_dir(root);
    std::fs::write(hist.join("WRONG.json"), crate::fold::render(&sample())).unwrap();
    let err = Export::load(root, DEFAULT_BASE_URL)
        .unwrap_err()
        .to_string();
    assert!(err.contains("WRONG.json cannot be read: it is the history of DISPATCH_UNIT_SCADA, not of its file name"), "{err}");
    std::fs::write(hist.join("WRONG.json"), "{}").unwrap();
    assert!(Export::load(root, DEFAULT_BASE_URL).is_err());
    std::fs::remove_file(hist.join("WRONG.json")).unwrap();
    std::fs::create_dir(hist.join("DIR.json")).unwrap();
    assert!(Export::load(root, DEFAULT_BASE_URL).is_err());
    let flat = tempfile::tempdir().unwrap();
    crate::files::write_atomic(&flat.path().join("evidence/histories"), b"x").unwrap();
    let err = read_histories(flat.path()).unwrap_err().to_string();
    assert!(
        err.starts_with("The export could not read or write ") && err.ends_with("histories."),
        "{err}"
    );
    // No histories: an export of nothing.
    let empty = tempfile::tempdir().unwrap();
    std::fs::create_dir(empty.path().join("tables")).unwrap();
    let r = Export::load(empty.path(), "b/").unwrap().render();
    assert_eq!(
        r.files.keys().collect::<Vec<_>>(),
        ["manifest.json", "nem/headers.json"]
    );

    // Declarations and releases that do not read.
    std::fs::remove_dir(hist.join("DIR.json")).unwrap();
    std::fs::write(root.join("tables/BAD.toml"), "x").unwrap();
    assert!(
        Export::load(root, DEFAULT_BASE_URL)
            .unwrap_err()
            .to_string()
            .contains("BAD.toml")
    );
    std::fs::remove_file(root.join("tables/BAD.toml")).unwrap();
    std::fs::write(root.join("evidence/models/5.8.toml"), "x").unwrap();
    assert!(
        Export::load(root, DEFAULT_BASE_URL)
            .unwrap_err()
            .to_string()
            .contains("5.8.toml")
    );
}

// ── The consumer ─────────────────────────────────────────────────────────
//
// What pigface-io's `crates/schemaviz-index/src/import.rs` (`import`,
// `read_table`) requires of an export, check by check, with the lines of
// that file each mirrors. It returns the tables read, or the first
// requirement an export breaks.

fn get<'a>(v: &'a serde_json::Value, key: &str, at: &str) -> Result<&'a serde_json::Value, String> {
    v.get(key)
        .ok_or_else(|| format!("{at}: {key:?} is missing"))
}

fn opt<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    v.get(key).filter(|v| !v.is_null())
}

fn s<'a>(v: &'a serde_json::Value, at: &str) -> Result<&'a str, String> {
    v.as_str()
        .ok_or_else(|| format!("{at}: {v} is not a string"))
}

fn u(v: &serde_json::Value, at: &str) -> Result<u64, String> {
    v.as_u64()
        .ok_or_else(|| format!("{at}: {v} is not a whole number"))
}

fn u32_(v: &serde_json::Value, at: &str) -> Result<u32, String> {
    u32::try_from(u(v, at)?).map_err(|_| format!("{at}: {v} is not a 32-bit number"))
}

fn arr<'a>(v: &'a serde_json::Value, at: &str) -> Result<&'a Vec<serde_json::Value>, String> {
    v.as_array()
        .ok_or_else(|| format!("{at}: {v} is not a list"))
}

fn strings(v: &serde_json::Value, at: &str) -> Result<Vec<String>, String> {
    arr(v, at)?
        .iter()
        .map(|x| s(x, at).map(str::to_owned))
        .collect()
}

fn opt_str(v: &serde_json::Value, key: &str, at: &str) -> Result<(), String> {
    opt(v, key).map(|x| s(x, at)).transpose().map(drop)
}

/// import.rs 375-388 (`mark`), with `time_of` (161-170) and `month_start`
/// (364-373).
fn check_mark(m: &serde_json::Value, at: &str) -> Result<(), String> {
    if let Some(r) = opt(m, "release") {
        let r = s(r, at)?;
        let ok = r.len() == 7
            && r.as_bytes()[4] == b'-'
            && r[5..].parse::<u32>().is_ok_and(|m| (1..=12).contains(&m));
        ok.then_some(())
            .ok_or_else(|| format!("{at}: {r:?} is not a month"))?;
    }
    s(get(m, "file", at)?, at)?;
    let t = get(m, "data-time", at)?;
    if !t.is_null() {
        let t = s(t, at)?;
        crate::sql::parse_time(t)
            .filter(|_| t.len() >= 20)
            .ok_or_else(|| format!("{at}: {t:?} is not RFC 3339"))?;
    }
    Ok(())
}

/// import.rs 240-325 (`import`) and 391-583 (`read_table`).
fn import_check(dir: &Path) -> Result<usize, String> {
    // 241-255: the manifest, its format, base URL and hashes.
    let manifest = read(dir, "manifest.json");
    if u(get(&manifest, "format", "manifest.json")?, "manifest.json")? != 2 {
        return Err("manifest.json: format is not 2".into());
    }
    s(
        get(&manifest, "base_url", "manifest.json")?,
        "manifest.json",
    )?;
    let listed: BTreeMap<String, String> = get(&manifest, "sha256", "manifest.json")?
        .as_object()
        .ok_or("manifest.json: sha256 is not an object")?
        .iter()
        .map(|(k, v)| s(v, "manifest.json").map(|v| (k.clone(), v.to_owned())))
        .collect::<Result<_, _>>()?;
    // 257-263: every file under nem/ is listed.
    let present: BTreeSet<String> = crate::census::files_under(&dir.join("nem"))
        .unwrap()
        .iter()
        .map(|p| {
            p.strip_prefix(dir)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    if let Some(p) = present.iter().find(|p| !listed.contains_key(*p)) {
        return Err(format!("{p}: is under nem/ but not in the manifest"));
    }
    // 265-275: every listed file is there with its hash, and JSON when it
    // is a `.json`.
    let mut docs = BTreeMap::new();
    for (rel, want) in &listed {
        let bytes = std::fs::read(dir.join(rel)).map_err(|e| format!("{rel}: {e}"))?;
        if crate::ctl::sha256(&bytes) != *want {
            return Err(format!("{rel}: the SHA-256 is not the manifest's"));
        }
        if !rel.ends_with(".json") {
            continue;
        }
        let v: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("{rel}: {e}"))?;
        docs.insert(rel.clone(), v);
    }
    let mut tables = 0;
    for (rel, doc) in docs.iter().filter(|(k, _)| k.ends_with("/table.json")) {
        let at = rel.as_str();
        // 299-309: named for its directory, x-format 2.
        let name = s(get(doc, "name", at)?, at)?;
        if *rel != format!("nem/{name}/table.json") {
            return Err(format!("{at}: names table {name:?}"));
        }
        if u(get(doc, "x-format", at)?, at)? != 2 {
            return Err(format!("{at}: x-format is not 2"));
        }
        // 397-401: declared, the iceberg block, format version 3.
        let declared = get(doc, "declared", at)?
            .as_bool()
            .ok_or(format!("{at}: declared"))?;
        let iceberg = get(doc, "iceberg", at)?;
        if u(get(iceberg, "format-version", at)?, at)? != 3 {
            return Err(format!("{at}: iceberg.format-version is not 3"));
        }
        // 345-360: columns, each an id, a name, a comment and a model type.
        if let Some(cols) = opt(doc, "columns") {
            for c in arr(cols, at)? {
                u32_(get(c, "id", at)?, at)?;
                s(get(c, "name", at)?, at)?;
                opt_str(c, "comment", at)?;
                opt_str(c, "model_type", at)?;
            }
        }
        // 403-475: schemas, fields of one name and type each, and each
        // schema's document listed and naming the schema and table.
        let files = get(iceberg, "schema-files", at)?;
        let mut fields: BTreeMap<u32, (String, String)> = BTreeMap::new();
        for sc in arr(get(iceberg, "schemas", at)?, at)? {
            let id = u32_(get(sc, "schema-id", at)?, at)?;
            if let Some(k) = opt(sc, "identifier-field-ids") {
                for i in arr(k, at)? {
                    u32_(i, at)?;
                }
            }
            for f in arr(get(sc, "fields", at)?, at)? {
                let fid = u32_(get(f, "id", at)?, at)?;
                let fname = s(get(f, "name", at)?, at)?.to_owned();
                let ty = s(get(f, "type", at)?, at)?.to_owned();
                utilake_iceberg::IcebergType::parse(&ty)
                    .ok_or(format!("{at}: {ty:?} is not a field type"))?;
                get(f, "required", at)?
                    .as_bool()
                    .ok_or(format!("{at}: required"))?;
                let had = fields.entry(fid).or_insert((fname.clone(), ty.clone()));
                if *had != (fname, ty) {
                    return Err(format!("{at}: field {fid} has two names or types"));
                }
            }
            let file = s(get(files, &id.to_string(), at)?, at)?;
            let doc_rel = format!("nem/{name}/{file}");
            let sdoc = docs
                .get(&doc_rel)
                .ok_or(format!("{at}: {doc_rel} is not in the manifest"))?;
            let xt = get(sdoc, "x-table", &doc_rel)?;
            if u(get(sdoc, "x-schema-id", &doc_rel)?, &doc_rel)? != u64::from(id)
                || s(get(xt, "name", &doc_rel)?, &doc_rel)? != name
            {
                return Err(format!("{doc_rel}: is not schema {id} of {name}"));
            }
        }
        // 476-490: a declared key names fields.
        if declared {
            for k in strings(get(doc, "primary_key", at)?, at)? {
                if !fields.values().any(|f| f.0 == k) {
                    return Err(format!("{at}: key column {k:?} is in no schema"));
                }
            }
        }
        // 491-552: runs.
        for r in arr(get(iceberg, "runs", at)?, at)? {
            if strings(get(r, "report", at)?, at)?.len() != 2 {
                return Err(format!("{at}: a run's report is not [type, subtype]"));
            }
            for c in strings(get(r, "changes", at)?, at)? {
                if !["first", "schema", "version", "report", "published"].contains(&c.as_str()) {
                    return Err(format!("{at}: {c:?} is not a change"));
                }
            }
            for up in arr(get(r, "updates", at)?, at)? {
                let a = s(get(up, "action", at)?, at)?;
                match a {
                    "set-identifier-fields" => {
                        for i in arr(get(up, "field-ids", at)?, at)? {
                            u32_(i, at)?;
                        }
                    }
                    "add" | "restore" | "delete" | "move" | "make-optional" => {
                        u32_(get(up, "field-id", at)?, at)?;
                    }
                    _ => return Err(format!("{at}: {a:?} is not a schema update")),
                }
            }
            u32_(get(r, "schema-id", at)?, at)?;
            u16::try_from(u(get(r, "version", at)?, at)?).map_err(|_| format!("{at}: version"))?;
            strings(get(r, "kinds", at)?, at)?;
            check_mark(get(r, "from", at)?, at)?;
            check_mark(get(r, "to", at)?, at)?;
            u32_(get(r, "files", at)?, at)?;
            u(get(r, "rows", at)?, at)?;
            get(r, "overlaps", at)?
                .as_bool()
                .ok_or(format!("{at}: overlaps"))?;
        }
        // 553-564: breaking columns.
        for b in arr(get(iceberg, "breaking", at)?, at)? {
            u32_(get(b, "old-field-id", at)?, at)?;
            u32_(get(b, "new-field-id", at)?, at)?;
            check_mark(get(b, "at", at)?, at)?;
        }
        // 565-576 with `kind` (219-237): description, package, time column;
        // a declared table's temporal roles and load mode.
        opt_str(doc, "description", at)?;
        opt_str(doc, "package", at)?;
        opt_str(doc, "time_column", at)?;
        if declared {
            if let Some(t) = opt(doc, "temporal") {
                s(get(get(t, "valid", at)?, "kind", at)?, at)?;
                strings(get(t, "order", at)?, at)?;
            }
            s(get(doc, "load_mode", at)?, at)?;
        }
        tables += 1;
    }
    // 316-318: at least one table.
    if tables == 0 {
        return Err("manifest.json: names no table".into());
    }
    Ok(tables)
}

#[test]
fn the_export_reads_as_the_index_reads_it() {
    let dir = exported();
    let root = dir.path();
    assert_eq!(import_check(root), Ok(1));
    grow(root);
    assert_eq!(import_check(root), Ok(1));
}

#[test]
fn the_consumer_check_refuses_what_the_index_refuses() {
    let dir = exported();
    let root = dir.path();
    let table = "nem/DISPATCH_UNIT_SCADA/table.json";
    // Each edit of table.json, rehashed into the manifest, and what the
    // check says.
    type Edit = Box<dyn Fn(&mut serde_json::Value)>;
    let cases: Vec<(Edit, &str)> = vec![
        (Box::new(|t| t["name"] = json!("X")), "names table"),
        (Box::new(|t| t["x-format"] = json!(1)), "x-format is not 2"),
        (
            Box::new(|t| t["iceberg"]["format-version"] = json!(2)),
            "format-version is not 3",
        ),
        (
            Box::new(|t| t["columns"][0]["id"] = json!(-1)),
            "is not a whole number",
        ),
        (
            Box::new(|t| t["columns"][0]["id"] = json!(5_000_000_000_u64)),
            "is not a 32-bit number",
        ),
        (Box::new(|t| t["columns"] = json!({})), "is not a list"),
        (
            Box::new(|t| t["columns"][0]["comment"] = json!(1)),
            "is not a string",
        ),
        (
            Box::new(|t| t["iceberg"]["schemas"][0]["fields"][0]["type"] = json!("uuid")),
            "is not a field type",
        ),
        (
            Box::new(|t| t["iceberg"]["schemas"][1]["fields"][0]["name"] = json!("T")),
            "has two names or types",
        ),
        (
            Box::new(|t| t["iceberg"]["schemas"][0]["identifier-field-ids"] = json!(["a"])),
            "is not a whole number",
        ),
        (
            Box::new(|t| t["iceberg"]["schema-files"]["0"] = json!("schema-none.json")),
            "is not in the manifest",
        ),
        (
            Box::new(|t| {
                t["iceberg"]["schema-files"]["0"] = t["iceberg"]["schema-files"]["1"].clone();
            }),
            "is not schema 0",
        ),
        (
            Box::new(|t| t["primary_key"] = json!(["NOPE"])),
            "is in no schema",
        ),
        (
            Box::new(|t| t["iceberg"]["runs"][0]["report"] = json!(["A"])),
            "is not [type, subtype]",
        ),
        (
            Box::new(|t| t["iceberg"]["runs"][0]["changes"] = json!(["x"])),
            "is not a change",
        ),
        (
            Box::new(|t| t["iceberg"]["runs"][1]["updates"][0]["action"] = json!("x")),
            "is not a schema update",
        ),
        (
            Box::new(|t| t["iceberg"]["runs"][0]["version"] = json!(70_000)),
            "version",
        ),
        (
            Box::new(|t| t["iceberg"]["runs"][0]["from"]["release"] = json!("2026-13")),
            "is not a month",
        ),
        (
            Box::new(|t| t["iceberg"]["runs"][0]["from"]["data-time"] = json!("2026-08-01")),
            "is not RFC 3339",
        ),
        (
            Box::new(|t| t["iceberg"]["runs"][0]["overlaps"] = json!(1)),
            "overlaps",
        ),
        (
            Box::new(|t| t["iceberg"]["breaking"] = json!([{"old-field-id": 1}])),
            "\"new-field-id\" is missing",
        ),
        (Box::new(|t| t["declared"] = json!("yes")), "declared"),
        (
            Box::new(|t| t["temporal"] = json!({"valid": {}})),
            "\"kind\" is missing",
        ),
        (
            Box::new(|t| t["iceberg"]["schemas"][0]["fields"][0]["required"] = json!(null)),
            "required",
        ),
    ];
    let original = std::fs::read(root.join(table)).unwrap();
    let manifest = std::fs::read(root.join("manifest.json")).unwrap();
    for (edit, says) in cases {
        let mut t: serde_json::Value = serde_json::from_slice(&original).unwrap();
        edit(&mut t);
        let text = text(&t);
        std::fs::write(root.join(table), &text).unwrap();
        let mut m: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
        m["sha256"][table] = json!(crate::ctl::sha256(text.as_bytes()));
        std::fs::write(root.join("manifest.json"), super::text(&m)).unwrap();
        let e = import_check(root).unwrap_err();
        assert!(e.contains(says), "{says}: {e}");
    }
    // The manifest and the files under nem/.
    std::fs::write(root.join(table), &original).unwrap();
    let mut m: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    m["format"] = json!(1);
    std::fs::write(root.join("manifest.json"), text(&m)).unwrap();
    assert!(import_check(root).unwrap_err().contains("format is not 2"));
    std::fs::write(root.join("manifest.json"), &manifest).unwrap();
    std::fs::write(root.join("nem/extra.json"), "{}").unwrap();
    assert!(
        import_check(root)
            .unwrap_err()
            .contains("not in the manifest")
    );
    std::fs::remove_file(root.join("nem/extra.json")).unwrap();
    std::fs::write(root.join(table), b"{}").unwrap();
    assert!(
        import_check(root)
            .unwrap_err()
            .contains("not the manifest's")
    );
    let empty = tempfile::tempdir().unwrap();
    std::fs::create_dir(empty.path().join("tables")).unwrap();
    export(empty.path());
    assert_eq!(
        import_check(empty.path()),
        Err("manifest.json: names no table".into())
    );
}
