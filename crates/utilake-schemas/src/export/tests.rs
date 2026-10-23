use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;
use utilake_iceberg::table::{TableMetadata, read_manifest_list};

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

const TABLE: &str = "nem/DISPATCH_UNIT_SCADA/metadata/v1.metadata.json";

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
             "updates": [{"action": "move", "field-id": 3}],
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
    put("tables/nem/DISPATCH_UNIT_SCADA.toml", SCADA);
    put("evidence/models/nem/5.6.toml", MODEL_56);
    put("evidence/models/nem/5.7.toml", MODEL_57);
    put(
        "evidence/histories/nem/DISPATCH_UNIT_SCADA.json",
        &crate::fold::render(&sample()),
    );
    export(root);
    dir
}

fn export(root: &Path) -> Rendered {
    let r = Export::load(root, DEFAULT_BASE_URL)
        .unwrap()
        .render()
        .unwrap();
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
        &history_path(root, Market::Nem, &h.table),
        crate::fold::render(&h).as_bytes(),
    )
    .unwrap();
    export(root);
}

fn read(root: &Path, rel: &str) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(root.join(rel)).unwrap()).unwrap()
}

#[test]
fn the_metadata_describes_the_table_and_holds_its_history() {
    let dir = exported();
    let root = dir.path();
    let m = read(root, TABLE);
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
    // The declaration and the data model's words are properties.
    let p = &m["properties"];
    assert_eq!(p["utilake.format"], "3");
    assert_eq!(p["utilake.table"], "DISPATCH_UNIT_SCADA");
    assert_eq!(p["utilake.declared"], "true");
    assert_eq!(
        p["comment"], "Unit SCADA",
        "the latest release with the table"
    );
    assert_eq!(p["utilake.package"], "DISPATCH");
    assert_eq!(p["utilake.key"], r#"["SETTLEMENTDATE","DUID"]"#);
    assert_eq!(p["utilake.reports"], r#"[["DISPATCH","UNIT_SCADA"]]"#);
    assert_eq!(p["utilake.time"], "SETTLEMENTDATE");
    assert_eq!(p["utilake.load"], "append");
    assert_eq!(p["utilake.visibility"], "PUBLIC");
    assert_eq!(p["utilake.temporal.valid.kind"], "interval_end");
    assert_eq!(p["utilake.temporal.order"], r#"["LASTCHANGED"]"#);
    // Only 5.6 has SCADAVALUE: the latest release with the column.
    assert_eq!(
        p["utilake.model-types"],
        r#"{"1":"TIMESTAMP(0)","2":"VARCHAR(20)","3":"NUMERIC(16,6)"}"#
    );
    assert!(p.get("utilake.breaks").is_none());
    // Each column's words are its field's doc, on every schema's copy.
    for s in m["schemas"].as_array().unwrap() {
        for f in s["fields"].as_array().unwrap() {
            let doc = f.get("doc").and_then(|d| d.as_str());
            match f["id"].as_u64().unwrap() {
                1 => assert_eq!(doc, Some("Interval end")),
                2 => assert_eq!(doc, None, "no words for DUID"),
                _ => assert_eq!(doc, Some("MW")),
            }
        }
    }
    // The runs are the snapshots, each with an empty manifest list of its
    // own under nem/.
    let snaps = m["snapshots"].as_array().unwrap();
    assert_eq!(snaps.len(), 2);
    assert_eq!(snaps[0]["summary"]["utilake.report-subtype"], "UNIT_SCADA");
    assert_eq!(snaps[1]["summary"]["utilake.version"], "2");
    assert_eq!(snaps[1]["schema-id"], 1);
    assert_eq!(m["current-snapshot-id"], snaps[1]["snapshot-id"]);
    for s in snaps {
        let list = s["manifest-list"].as_str().unwrap();
        let rel = list.strip_prefix(DEFAULT_BASE_URL).unwrap();
        assert!(
            rel.starts_with("nem/DISPATCH_UNIT_SCADA/metadata/snap-"),
            "{rel}"
        );
        let bytes = std::fs::read(root.join(rel)).unwrap();
        assert_eq!(read_manifest_list(&bytes).unwrap(), []);
    }
    // Every header, with its table and schemas.
    let hs = read(root, "nem/headers.json");
    assert_eq!(hs["x-format"], 3);
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
    // The history as the fold wrote it comes back out of the metadata,
    // and each schema's document is there by id.
    let meta: TableMetadata = serde_json::from_value(m).unwrap();
    assert_eq!(from_metadata(&meta).unwrap().history, h);
    for s in &h.schemas {
        let name = schema::file_name(s.schema_id);
        assert!(root.join("nem/DISPATCH_UNIT_SCADA").join(&name).is_file());
    }
    let published = crate::fold::published(root, Market::Nem).unwrap();
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
fn a_schema_document_is_named_by_its_id_and_its_fields_never_change() {
    let dir = exported();
    let root = dir.path();
    assert_eq!(schema::file_name(0), "schema-0.json");
    let rel = "nem/DISPATCH_UNIT_SCADA/schema-0.json";
    let before = std::fs::read(root.join(rel)).unwrap();
    let d: serde_json::Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(d["$id"], format!("{DEFAULT_BASE_URL}{rel}"));
    assert_eq!(d["$schema"], "https://json-schema.org/draft/2020-12/schema");
    assert_eq!(d["x-format"], 3);
    assert_eq!(
        d["x-table"],
        json!({"name": "DISPATCH_UNIT_SCADA", "$ref": "metadata/v1.metadata.json"})
    );
    assert_eq!(d["x-schema-id"], 0);
    assert_eq!(d["x-identifier-field-ids"], json!([1, 2]));
    assert_eq!(d["x-fields"][2], json!([3, "SCADAVALUE", "decimal(16, 6)"]));
    assert_eq!(d["required"], json!(["SETTLEMENTDATE", "DUID"]));
    assert_eq!(d["additionalProperties"], false);
    let p = &d["properties"];
    assert_eq!(
        p["SETTLEMENTDATE"],
        json!({"type": "string", "format": "date-time", "description": "Interval end",
               "x-field-id": 1, "x-iceberg-type": "timestamptz", "x-model-type": "TIMESTAMP(0)"})
    );
    assert_eq!(p["SCADAVALUE"]["type"], json!(["number", "null"]));
    assert_eq!(p["DUID"].get("description"), None);
    assert!(d.get("x-runs").is_none());
    grow(root);
    assert_eq!(
        std::fs::read(root.join(rel)).unwrap(),
        before,
        "more runs, the same bytes"
    );
    let p = Export::load(root, DEFAULT_BASE_URL)
        .unwrap()
        .published(Market::Nem, "DISPATCH_UNIT_SCADA")
        .unwrap();
    let types: Vec<serde_json::Value> = ["boolean", "long", "double", "date", "string"]
        .iter()
        .map(|t| {
            let s = one_column(t);
            let doc: serde_json::Value =
                serde_json::from_str(&schema::document("b/", "T", "p", &s, &p)).unwrap();
            doc["properties"]["C"].clone()
        })
        .collect();
    assert_eq!(
        types,
        [
            json!({"type": "boolean", "x-field-id": 9, "x-iceberg-type": "boolean"}),
            json!({"type": ["integer", "null"], "x-field-id": 9, "x-iceberg-type": "long"}),
            json!({"type": ["number", "null"], "x-field-id": 9, "x-iceberg-type": "double"}),
            json!({"type": ["string", "null"], "format": "date", "x-field-id": 9, "x-iceberg-type": "date"}),
            json!({"type": ["string", "null"], "x-field-id": 9, "x-iceberg-type": "string"}),
        ]
    );
}

/// A schema of one column `C` (field 9, undocumented) of type `ty`,
/// required when boolean.
fn one_column(ty: &str) -> utilake_iceberg::Schema {
    utilake_iceberg::Schema::from_json(
        &json!({"schema-id": 0, "fields": [field(9, "C", ty, ty == "boolean")]}),
    )
    .unwrap()
}

#[test]
fn the_manifest_names_every_file_and_the_export_owns_nem() {
    let dir = exported();
    let root = dir.path();
    let m = read(root, "manifest.json");
    assert_eq!(m["format"], 3);
    assert!(m.get("market").is_none());
    assert_eq!(m["base_url"], DEFAULT_BASE_URL);
    let nem = &m["markets"]["nem"];
    assert_eq!(nem["baseline"], json!({"version": 1, "reason": ""}));
    assert_eq!(
        (nem["tables"].clone(), nem["schemas"].clone()),
        (json!(1), json!(2))
    );
    assert_eq!(
        m["markets"].as_object().unwrap().len(),
        1,
        "only the NEM is published"
    );
    assert_eq!(
        (m["tables"].clone(), m["schemas"].clone()),
        (json!(1), json!(2))
    );
    // A re-based root says so.
    std::fs::write(
        crate::fold::baseline_path(root, Market::Nem),
        "version = 2\nreason = \"why\"\n",
    )
    .unwrap();
    let r = Export::load(root, DEFAULT_BASE_URL)
        .unwrap()
        .render()
        .unwrap();
    let m: serde_json::Value = serde_json::from_slice(&r.files["manifest.json"]).unwrap();
    assert_eq!(
        m["markets"]["nem"]["baseline"],
        json!({"version": 2, "reason": "why"})
    );
    std::fs::write(crate::fold::baseline_path(root, Market::Nem), "version = ").unwrap();
    assert!(Export::load(root, DEFAULT_BASE_URL).is_err());
    std::fs::remove_file(crate::fold::baseline_path(root, Market::Nem)).unwrap();
    assert_eq!(
        m["markets"]["nem"]["listed"],
        json!([{"name": "DISPATCH_UNIT_SCADA", "package": "DISPATCH", "declared": true,
                "first_release": "2026-08", "last_release": "2026-08",
                "schemas": 2, "runs": 2, "breaks": 0}])
    );
    // Metadata, its hint, two manifest lists, two schema documents, and
    // the headers.
    let listed = m["sha256"].as_object().unwrap();
    assert_eq!(listed.len(), 7);
    assert_eq!(listed.keys().filter(|k| k.ends_with(".avro")).count(), 2);
    for (p, sha) in listed {
        assert_eq!(
            crate::ctl::sha256(&std::fs::read(root.join(p)).unwrap()),
            *sha
        );
    }
    assert!(!root.join("provenance.json").exists());

    // Again: the same bytes, nothing written.
    let r = Export::load(root, DEFAULT_BASE_URL)
        .unwrap()
        .render()
        .unwrap();
    assert_eq!(
        r,
        Export::load(root, DEFAULT_BASE_URL)
            .unwrap()
            .render()
            .unwrap()
    );
    assert!(check(root, &r).unwrap().is_empty());
    assert_eq!(
        write(root, &r).unwrap(),
        Written {
            written: 0,
            kept: 8,
            removed: 0
        }
    );

    // Stale: a file of nobody's, one edited, one gone.
    std::fs::create_dir_all(root.join("nem/OLD/deep")).unwrap();
    std::fs::write(root.join("nem/OLD/deep/x.json"), "{}").unwrap();
    std::fs::write(root.join("nem/DISPATCH_UNIT_SCADA/table.json"), "{}").unwrap();
    std::fs::write(root.join("manifest.json"), "{}").unwrap();
    std::fs::remove_file(root.join(TABLE)).unwrap();
    let stale: Vec<String> = check(root, &r)
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        stale,
        [
            "manifest.json differs",
            "nem/DISPATCH_UNIT_SCADA/metadata/v1.metadata.json missing",
            "nem/DISPATCH_UNIT_SCADA/table.json extra",
            "nem/OLD/deep/x.json extra"
        ]
    );
    assert_eq!(
        write(root, &r).unwrap(),
        Written {
            written: 2,
            kept: 6,
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
        &history_path(root, Market::Nem, "OTHER"),
        crate::fold::render(&other).as_bytes(),
    )
    .unwrap();
    std::fs::write(
        crate::fold::histories_dir(root, Market::Nem).join("notes.txt"),
        "not a history",
    )
    .unwrap();
    let r = export(root);
    assert_eq!((r.tables, r.schemas), (2, 4));
    let t = read(root, "nem/OTHER/metadata/v1.metadata.json");
    let p = t["properties"].as_object().unwrap();
    assert_eq!(p["utilake.declared"], "false");
    for k in [
        "comment",
        "utilake.key",
        "utilake.reports",
        "utilake.time",
        "utilake.temporal.valid.kind",
        "utilake.load",
        "utilake.visibility",
        "utilake.model-types",
    ] {
        assert!(!p.contains_key(k), "{k}");
    }
    assert!(t["schemas"][0]["fields"][0].get("doc").is_none());
    let d = read(root, "nem/OTHER/schema-0.json");
    assert!(
        d["properties"]["SETTLEMENTDATE"]
            .get("description")
            .is_none()
    );
    let m = read(root, "manifest.json");
    assert_eq!(m["listed"][1]["package"], json!(null));
    assert_eq!(import_check(root), Ok(2));
}

#[test]
fn what_cannot_be_read_or_written_is_named() {
    let dir = exported();
    let root = dir.path();
    let r = Export::load(root, DEFAULT_BASE_URL)
        .unwrap()
        .render()
        .unwrap();
    let e = |x: Result<Written, ExportError>| x.unwrap_err().to_string();
    // A directory where a file is to be.
    let table = root.join(TABLE);
    std::fs::remove_file(&table).unwrap();
    std::fs::create_dir(&table).unwrap();
    assert!(
        check(root, &r)
            .unwrap_err()
            .to_string()
            .ends_with("v1.metadata.json.")
    );
    assert!(e(write(root, &r)).ends_with("v1.metadata.json."));
    std::fs::remove_dir(&table).unwrap();
    // A file where nem/ is.
    let flat = tempfile::tempdir().unwrap();
    std::fs::write(flat.path().join("nem"), "x").unwrap();
    assert!(check(flat.path(), &Rendered::default()).is_err());
    assert!(e(write(flat.path(), &Rendered::default())).contains("nem"));

    // Histories: misnamed, not a history, not a directory.
    let hist = crate::fold::histories_dir(root, Market::Nem);
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
    crate::files::write_atomic(&flat.path().join("evidence/histories/nem"), b"x").unwrap();
    let err = read_histories(flat.path(), Market::Nem)
        .unwrap_err()
        .to_string();
    assert!(
        err.starts_with("The export could not read or write ") && err.ends_with("histories/nem."),
        "{err}"
    );
    assert!(Export::load(flat.path(), DEFAULT_BASE_URL).is_err());
    // No histories: an export of nothing.
    let empty = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(empty.path().join("tables/nem")).unwrap();
    let r = Export::load(empty.path(), "b/").unwrap().render().unwrap();
    assert_eq!(
        r.files.keys().collect::<Vec<_>>(),
        ["manifest.json", "nem/headers.json"]
    );
    assert!(
        Export::load(empty.path(), "b/")
            .unwrap()
            .published(Market::Nem, "X")
            .is_none()
    );
    assert!(
        Export::load(empty.path(), "b/")
            .unwrap()
            .table(Market::Nem, "X", "l")
            .is_none()
    );

    // A history that cannot be published: a run released in no month.
    std::fs::remove_dir(hist.join("DIR.json")).unwrap();
    let mut h = sample();
    h.runs[0].from.release = "August".into();
    crate::files::write_atomic(
        &history_path(root, Market::Nem, &h.table),
        crate::fold::render(&h).as_bytes(),
    )
    .unwrap();
    let err = Export::load(root, DEFAULT_BASE_URL)
        .unwrap()
        .render()
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "The history of DISPATCH_UNIT_SCADA cannot be published: The run beginning at A.zip was released in \"August\", which is not a month (YYYY-MM)."
    );
    crate::files::write_atomic(
        &history_path(root, Market::Nem, "DISPATCH_UNIT_SCADA"),
        crate::fold::render(&sample()).as_bytes(),
    )
    .unwrap();

    // Declarations and releases that do not read, and a declaration whose
    // temporal roles the published form does not name.
    std::fs::write(root.join("tables/nem/BAD.toml"), "x").unwrap();
    assert!(
        Export::load(root, DEFAULT_BASE_URL)
            .unwrap_err()
            .to_string()
            .contains("BAD.toml")
    );
    std::fs::write(
        root.join("tables/nem/BAD.toml"),
        SCADA
            .replace("DISPATCH_UNIT_SCADA", "BAD")
            .replace("UNIT_SCADA", "BAD")
            .replace("cutoff = ", "horizon = "),
    )
    .unwrap();
    let err = Export::load(root, DEFAULT_BASE_URL)
        .unwrap_err()
        .to_string();
    assert!(
        err.starts_with("The declaration of BAD cannot be published: its temporal roles are not the published ones: "),
        "{err}"
    );
    std::fs::remove_file(root.join("tables/nem/BAD.toml")).unwrap();
    std::fs::write(root.join("evidence/models/nem/5.8.toml"), "x").unwrap();
    assert!(
        Export::load(root, DEFAULT_BASE_URL)
            .unwrap_err()
            .to_string()
            .contains("5.8.toml")
    );
}

// ── The consumer ─────────────────────────────────────────────────────────
//
// What pigface-io's `crates/schemaviz-index/src/import.rs` requires of an
// export, check by check. It returns the tables read, or the first
// requirement an export breaks.

fn get<'a>(v: &'a serde_json::Value, key: &str, at: &str) -> Result<&'a serde_json::Value, String> {
    v.get(key)
        .ok_or_else(|| format!("{at}: {key:?} is missing"))
}

fn s<'a>(v: &'a serde_json::Value, at: &str) -> Result<&'a str, String> {
    v.as_str()
        .ok_or_else(|| format!("{at}: {v} is not a string"))
}

fn u(v: &serde_json::Value, at: &str) -> Result<u64, String> {
    v.as_u64()
        .ok_or_else(|| format!("{at}: {v} is not a whole number"))
}

/// The importer, check by check.
fn import_check(dir: &Path) -> Result<usize, String> {
    // The manifest: its format, base URL and hashes.
    let manifest = read(dir, "manifest.json");
    if u(get(&manifest, "format", "manifest.json")?, "manifest.json")? != 3 {
        return Err("manifest.json: format is not 3".into());
    }
    let base = s(
        get(&manifest, "base_url", "manifest.json")?,
        "manifest.json",
    )?;
    let listed: BTreeMap<String, String> = get(&manifest, "sha256", "manifest.json")?
        .as_object()
        .ok_or("manifest.json: sha256 is not an object")?
        .iter()
        .map(|(k, v)| s(v, "manifest.json").map(|v| (k.clone(), v.to_owned())))
        .collect::<Result<_, _>>()?;
    // Every file under a market's directory is listed.
    let present: BTreeSet<String> = owned()
        .flat_map(|m| crate::census::files_under(&dir.join(m)).unwrap_or_default())
        .map(|p| {
            p.strip_prefix(dir)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    if let Some(p) = present.iter().find(|p| !listed.contains_key(*p)) {
        return Err(format!(
            "{p}: is under a market directory but not in the manifest"
        ));
    }
    // Every listed file is there with its hash; JSON parses, Avro is a
    // manifest list.
    let mut docs = BTreeMap::new();
    let mut lists = BTreeMap::new();
    for (rel, want) in &listed {
        let bytes = std::fs::read(dir.join(rel)).map_err(|e| format!("{rel}: {e}"))?;
        if crate::ctl::sha256(&bytes) != *want {
            return Err(format!("{rel}: the SHA-256 is not the manifest's"));
        }
        if rel.ends_with(".json") {
            let v: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| format!("{rel}: {e}"))?;
            docs.insert(rel.clone(), v);
        } else if rel.ends_with(".avro") {
            let entries = read_manifest_list(&bytes).map_err(|e| format!("{rel}: {e}"))?;
            lists.insert(rel.clone(), entries.len());
        }
    }
    let mut tables = 0;
    for (rel, doc) in docs
        .iter()
        .filter(|(k, _)| k.ends_with("/metadata/v1.metadata.json"))
    {
        let at = rel.as_str();
        // The metadata reads as a published table, named for its
        // directory, format version 3.
        let m: TableMetadata =
            serde_json::from_value(doc.clone()).map_err(|e| format!("{at}: {e}"))?;
        let p = from_metadata(&m).map_err(|e| format!("{at}: {e}"))?;
        let name = p.history.table.as_str();
        let market = p.market;
        if *rel != format!("{market}/{name}/metadata/v1.metadata.json") {
            return Err(format!("{at}: names table {name:?} of {market}"));
        }
        if m.format_version != 3 {
            return Err(format!("{at}: format-version is not 3"));
        }
        // Fields of one name and type each; each schema's document listed
        // and naming the schema and table.
        let mut fields: BTreeMap<u32, (String, String)> = BTreeMap::new();
        for sc in &p.history.schemas {
            for f in &sc.fields {
                let had = fields.entry(f.id).or_insert((f.name.clone(), f.ty.name()));
                if *had != (f.name.clone(), f.ty.name()) {
                    return Err(format!("{at}: field {} has two names or types", f.id));
                }
            }
            let doc_rel = format!("{market}/{name}/schema-{}.json", sc.schema_id);
            let sdoc = docs
                .get(&doc_rel)
                .ok_or(format!("{at}: {doc_rel} is not in the manifest"))?;
            let xt = get(sdoc, "x-table", &doc_rel)?;
            if u(get(sdoc, "x-schema-id", &doc_rel)?, &doc_rel)? != u64::from(sc.schema_id)
                || s(get(xt, "name", &doc_rel)?, &doc_rel)? != name
            {
                return Err(format!(
                    "{doc_rel}: is not schema {} of {name}",
                    sc.schema_id
                ));
            }
        }
        // A declared key names fields.
        if let Some(d) = &p.declaration {
            for k in &d.primary_key {
                if !fields.values().any(|f| f.0 == *k) {
                    return Err(format!("{at}: key column {k:?} is in no schema"));
                }
            }
        }
        // Each snapshot's manifest list is under the table, listed, and
        // empty.
        for sn in &m.snapshots {
            let list = sn
                .manifest_list
                .strip_prefix(base)
                .filter(|l| l.starts_with(&format!("{market}/{name}/metadata/")))
                .ok_or(format!("{at}: snapshot {} lists elsewhere", sn.snapshot_id))?;
            match lists.get(list) {
                Some(0) => {}
                Some(_) => return Err(format!("{list}: lists data files")),
                None => return Err(format!("{at}: {list} is not in the manifest")),
            }
        }
        tables += 1;
    }
    // At least one table.
    if tables == 0 {
        return Err("manifest.json: names no table".into());
    }
    Ok(tables)
}

#[test]
fn another_market_is_published_beside_the_nem_with_its_words_and_headers() {
    let dir = exported();
    let root = dir.path();
    let put = |rel: &str, text: &str| {
        crate::files::write_atomic(&root.join(rel), text.as_bytes()).unwrap();
    };
    put(
        "tables/gas/GBB_ACTUAL_FLOW_STORAGE.toml",
        "name = \"GBB_ACTUAL_FLOW_STORAGE\"\nreport_pairs = [[\"GBB\", \"ACTUAL_FLOW_STORAGE\"]]\nprimary_key = [\"GASDATE\"]\ntime_column = \"GASDATE\"\nload_mode = \"append\"\nvisibility = \"PUBLIC\"\nmodel = [\"GAS_DAILY_FLOW_ACTUAL\"]\n\n[temporal]\norigin = \"hand\"\nentity = []\nvalid = { kind = \"instant\", column = \"GASDATE\" }\norder = []\n",
    );
    put(
        "evidence/models/gas/2.2.toml",
        "version = \"2.2\"\nmodel = \"Gas Data Model Report\"\ncreated = \"\"\nscript_sha256 = \"g\"\nfirst_seen = \"2026-10\"\nlast_seen = \"2026-10\"\n\n[tables.GAS_DAILY_FLOW_ACTUAL]\ncomment = \"Daily flow\"\nprimary_key = [\"GASDATE\"]\ncolumns = [\n    [\"GasDate\", \"DATE\", false, \"Date of gas day\"],\n]\n",
    );
    let mut h = sample();
    h.table = "GBB_ACTUAL_FLOW_STORAGE".into();
    for s in &mut h.schemas {
        s.fields[0].name = "GASDATE".into();
    }
    h.headers = BTreeMap::from([(1, "GasDate".to_owned())]);
    put(
        "evidence/histories/gas/GBB_ACTUAL_FLOW_STORAGE.json",
        &crate::fold::render(&h),
    );
    let r = export(root);
    assert_eq!((r.tables, r.schemas), (2, 4));
    let t = read(
        root,
        "gas/GBB_ACTUAL_FLOW_STORAGE/metadata/v1.metadata.json",
    );
    let p = t["properties"].as_object().unwrap();
    assert_eq!(p["utilake.market"], "gas");
    assert_eq!(p["utilake.offset"], "+10:00");
    assert_eq!(p["utilake.declared"], "true");
    assert_eq!(p["utilake.model"], r#"["GAS_DAILY_FLOW_ACTUAL"]"#);
    assert_eq!(p["utilake.headers"], r#"{"1":"GasDate"}"#);
    assert_eq!(p["comment"], "Daily flow", "the model table's words");
    assert_eq!(p["utilake.model-types"], r#"{"1":"DATE"}"#);
    assert_eq!(t["schemas"][0]["fields"][0]["doc"], "Date of gas day");
    assert_eq!(
        t["table-uuid"],
        crate::iceberg::table_uuid_in(Market::Gas, "GBB_ACTUAL_FLOW_STORAGE")
    );
    let d = read(root, "gas/GBB_ACTUAL_FLOW_STORAGE/schema-0.json");
    assert_eq!(d["properties"]["GASDATE"]["x-header"], "GasDate");
    assert!(d["properties"]["DUID"].get("x-header").is_none());
    assert!(root.join("gas/headers.json").is_file());
    // No folder census recorded: no folders.json for either market.
    assert!(!root.join("gas/folders.json").exists());
    assert!(!root.join("nem/folders.json").exists());
    // A folder census of the GBB root and of one of its sub-folders: each
    // folder with the blocks it carried, their versions and releases.
    put(
        "evidence/census/gas/gbb/CURRENT/_root.toml",
        &format!(
            "{}{}{}{}",
            FOLDER_HEAD,
            block("ACTUAL_FLOW_STORAGE", 1, "2026-09"),
            block("ACTUAL_FLOW_STORAGE", 1, "2026-10"),
            block("ACTUAL_FLOW_STORAGE", 1, "2026-08")
        ),
    );
    put(
        "evidence/census/gas/gbb/CURRENT/ForecastUtilisation.toml",
        &format!(
            "{}{}{}",
            FOLDER_HEAD,
            block("FORECAST_UTILISATION", 2, "2026-10"),
            block("FORECAST_UTILISATION", 1, "2026-08")
        ),
    );
    export(root);
    let f = read(root, "gas/folders.json");
    assert_eq!(f["x-format"], 3);
    assert_eq!(
        f["folders"],
        json!([
            {"feed": "gbb", "label": "CURRENT", "root": "Reports/CURRENT/GBB", "folder": "GBB",
             "blocks": [{"report": ["GBB", "ACTUAL_FLOW_STORAGE"], "version": 1,
                         "first-release": "2026-08", "last-release": "2026-10", "months": 3}]},
            {"feed": "gbb", "label": "CURRENT", "root": "Reports/CURRENT/GBB", "folder": "ForecastUtilisation",
             "blocks": [{"report": ["GBB", "FORECAST_UTILISATION"], "version": 1,
                         "first-release": "2026-08", "last-release": "2026-08", "months": 1},
                        {"report": ["GBB", "FORECAST_UTILISATION"], "version": 2,
                         "first-release": "2026-10", "last-release": "2026-10", "months": 1}]},
        ])
    );
    assert!(read(root, "manifest.json")["sha256"]["gas/folders.json"].is_string());
    // A folder census that does not read stops the export.
    put(
        "evidence/census/gas/gbb/CURRENT/_root.toml",
        "files = 'x'\n",
    );
    assert!(matches!(
        Export::load(root, DEFAULT_BASE_URL),
        Err(ExportError::Census(_))
    ));
    put(
        "evidence/census/gas/gbb/CURRENT/_root.toml",
        &format!(
            "{}{}",
            FOLDER_HEAD,
            block("ACTUAL_FLOW_STORAGE", 1, "2026-09")
        ),
    );
    let r = export(root);
    let m = read(root, "manifest.json");
    assert_eq!(m["markets"]["gas"]["tables"], 1);
    assert_eq!(m["markets"]["nem"]["tables"], 1);
    assert_eq!(m["tables"], 2);
    // The NEM's table says nothing of its market.
    let nem = read(root, TABLE);
    assert!(nem["properties"].get("utilake.market").is_none());
    assert!(check(root, &r).unwrap().is_empty());
    assert_eq!(import_check(root).unwrap(), 2);
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
    // Each edit of the metadata, rehashed into the manifest, and what the
    // check says.
    type Edit = Box<dyn Fn(&mut serde_json::Value)>;
    let cases: Vec<(Edit, &str)> = vec![
        (
            Box::new(|t| t["properties"]["utilake.table"] = json!("X")),
            "names table",
        ),
        (
            Box::new(|t| t["properties"]["utilake.format"] = json!("2")),
            "not format 3",
        ),
        (
            Box::new(|t| t["format-version"] = json!(2)),
            "format-version is not 3",
        ),
        (
            Box::new(|t| t["schemas"][0]["fields"][0]["type"] = json!("uuid")),
            "not a schema these types hold",
        ),
        (
            Box::new(|t| t["schemas"][1]["fields"][0]["name"] = json!("T")),
            "has two names or types",
        ),
        (
            Box::new(|t| {
                let mut s = t["schemas"][1].clone();
                s["schema-id"] = json!(7);
                t["schemas"].as_array_mut().unwrap().push(s);
            }),
            "schema-7.json is not in the manifest",
        ),
        (
            Box::new(|t| t["properties"]["utilake.key"] = json!("[\"NOPE\"]")),
            "is in no schema",
        ),
        (
            Box::new(|t| t["properties"]["utilake.declared"] = json!("yes")),
            "utilake.declared",
        ),
        (
            Box::new(|t| t["snapshots"][0]["manifest-list"] = json!("file:///elsewhere.avro")),
            "lists elsewhere",
        ),
        (
            Box::new(|t| {
                let list = t["snapshots"][0]["manifest-list"].as_str().unwrap();
                let list = list.replace("snap-", "snap-x");
                t["snapshots"][0]["manifest-list"] = json!(list);
            }),
            "is not in the manifest",
        ),
        (
            Box::new(|t| t["snapshots"][0]["summary"]["utilake.rows"] = json!("many")),
            "not a number",
        ),
    ];
    let original = std::fs::read(root.join(TABLE)).unwrap();
    let manifest = std::fs::read(root.join("manifest.json")).unwrap();
    for (edit, says) in cases {
        let mut t: serde_json::Value = serde_json::from_slice(&original).unwrap();
        edit(&mut t);
        let text = text(&t);
        std::fs::write(root.join(TABLE), &text).unwrap();
        let mut m: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
        m["sha256"][TABLE] = json!(crate::ctl::sha256(text.as_bytes()));
        std::fs::write(root.join("manifest.json"), super::text(&m)).unwrap();
        let e = import_check(root).unwrap_err();
        assert!(e.contains(says), "{says}: {e}");
    }
    std::fs::write(root.join(TABLE), &original).unwrap();
    // A schema document of another schema or table.
    let doc = "nem/DISPATCH_UNIT_SCADA/schema-0.json";
    let original_doc = std::fs::read(root.join(doc)).unwrap();
    let mut d: serde_json::Value = serde_json::from_slice(&original_doc).unwrap();
    d["x-schema-id"] = json!(1);
    let text = text(&d);
    std::fs::write(root.join(doc), &text).unwrap();
    let mut m: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    m["sha256"][doc] = json!(crate::ctl::sha256(text.as_bytes()));
    std::fs::write(root.join("manifest.json"), super::text(&m)).unwrap();
    assert!(import_check(root).unwrap_err().contains("is not schema 0"));
    std::fs::write(root.join(doc), &original_doc).unwrap();
    m["sha256"][doc] = json!(crate::ctl::sha256(&original_doc));
    // A manifest list that is not one, or lists a data file.
    let list = m["sha256"]
        .as_object()
        .unwrap()
        .keys()
        .find(|k| k.ends_with(".avro"))
        .unwrap()
        .clone();
    let original_list = std::fs::read(root.join(&list)).unwrap();
    std::fs::write(root.join(&list), b"Obj\x01nope").unwrap();
    m["sha256"][&list] = json!(crate::ctl::sha256(b"Obj\x01nope"));
    std::fs::write(root.join("manifest.json"), super::text(&m)).unwrap();
    assert!(import_check(root).unwrap_err().contains(".avro: "));
    let full = utilake_iceberg::table::write_manifest_list(
        1,
        None,
        1,
        0,
        &[utilake_iceberg::table::ManifestFile {
            path: "m.avro".into(),
            length: 1,
            sequence_number: 1,
            min_sequence_number: 1,
            added_snapshot_id: 1,
            added_files: 1,
            added_rows: 1,
            first_row_id: None,
        }],
    )
    .unwrap();
    std::fs::write(root.join(&list), &full).unwrap();
    m["sha256"][&list] = json!(crate::ctl::sha256(&full));
    std::fs::write(root.join("manifest.json"), super::text(&m)).unwrap();
    assert!(import_check(root).unwrap_err().contains("lists data files"));
    std::fs::write(root.join(&list), &original_list).unwrap();
    // The manifest and the files under nem/.
    let mut m: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    m["format"] = json!(1);
    std::fs::write(root.join("manifest.json"), super::text(&m)).unwrap();
    assert!(import_check(root).unwrap_err().contains("format is not 3"));
    std::fs::write(root.join("manifest.json"), &manifest).unwrap();
    std::fs::write(root.join("nem/extra.json"), "{}").unwrap();
    assert!(
        import_check(root)
            .unwrap_err()
            .contains("not in the manifest")
    );
    std::fs::remove_file(root.join("nem/extra.json")).unwrap();
    std::fs::write(root.join(TABLE), b"{}").unwrap();
    assert!(
        import_check(root)
            .unwrap_err()
            .contains("not the manifest's")
    );
    let empty = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(empty.path().join("tables/nem")).unwrap();
    export(empty.path());
    assert_eq!(
        import_check(empty.path()),
        Err("manifest.json: names no table".into())
    );
}

/// The head of a folder census file.
const FOLDER_HEAD: &str = "files = 2\ncsvs = 2\nother = 0\n";

/// One tally of a folder census: a block of one GBB report in one month.
fn block(name: &str, version: u16, month: &str) -> String {
    format!(
        r#"
[[block]]
month = "{month}"
csvs = 1
first_csv = "GasBB.csv"
last_csv = "GasBB.csv"
shape = "ef44f4a7f758e85a"
report = ["GBB", "{name}"]
version = {version}
rows = 1
columns = ["GASDATE"]
ctl = "?"
kinds = "D"
nulls = [0]
int_digits = [0]
scale = [0]
sig_digits = [0]
max_len = [10]
min = ["2026/09/01"]
max = ["2026/09/01"]
"#
    )
}
