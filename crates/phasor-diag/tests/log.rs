//! A diagnostic logged as one record. One test, because a process has one
//! logger.
#![cfg(feature = "log")]

use linelog::{Capture, Level};
use phasor_diag::{Category, Diagnostic};

#[test]
fn a_diagnostic_is_one_record() {
    let lines = Capture::new();
    linelog::init()
        .env("PHASOR_DIAG_TEST_UNSET")
        .format_env("PHASOR_DIAG_TEST_UNSET_FORMAT")
        .default_level(Level::Trace)
        .sink(lines.clone())
        .install()
        .unwrap();
    Diagnostic::error(
        "csv.field_count_mismatch",
        Category::CSV,
        "the row has 8 fields",
    )
    .with_file("a.zip")
    .with_entry("A.CSV")
    .at(17, 170)
    .expected_observed("7 fields", "8 fields")
    .log();
    Diagnostic::warning("net.offline", Category::NET, "offline").log();
    let got = lines.lines();
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(
        got[0].contains(" ERROR diag the row has 8 fields code=csv.field_count_mismatch file=a.zip url= entry=A.CSV table= line=17 column= expected=\"7 fields\" observed=\"8 fields\""),
        "{got:?}"
    );
    assert!(
        got[1].contains(" WARN  diag offline code=net.offline"),
        "{got:?}"
    );
}
