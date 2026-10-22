"""Read every table's Iceberg metadata with pyiceberg, and hold it to its
history.

    uv run --with pyiceberg --with pyarrow --with duckdb python scripts/replay.py [DIR] [HISTORIES]

DIR (default target/iceberg, as `utilake-schemas iceberg --out DIR` wrote
it: the same tables the export publishes under nem/, at file locations)
holds one table per directory with a `metadata/` in it; HISTORIES (default
evidence/histories) holds each table's history. Every history must have a
table and every table a history. Each table is loaded as a pyiceberg
`StaticTable` from the metadata its `version-hint.text` names, and must
have the history's schemas by id, each with its fields' ids, names, types
and nullability in order and its key, and the history's current schema as
its current schema; one snapshot per run, in order, each under the run's
schema with the run's report pair, version, files and rows in its summary,
the last one current; and a scan of the current snapshot must open every
manifest list and return no rows under the current schema. DuckDB's
iceberg extension must read the same: `iceberg_scan` of the table returns
no rows with the current schema's columns, and `iceberg_snapshots` one
row per run. Exits 1 on the first table that does not, naming it.

The tables hold no data, so pyiceberg reads the files with a FileIO of
this script's own (`LocalFileIO`): no fsspec is needed. pyarrow is needed
for the scan, duckdb for the second reader.
"""

import json
import sys
from pathlib import Path

import duckdb
from pyiceberg.io import FileIO, InputFile, InputStream, OutputFile
from pyiceberg.table import StaticTable


class LocalInputFile(InputFile):
    """A local file, read whole."""

    def __len__(self) -> int:
        return Path(self.location).stat().st_size

    def exists(self) -> bool:
        return Path(self.location).exists()

    def open(self, seekable: bool = True) -> InputStream:
        return open(self.location, "rb")


class LocalFileIO(FileIO):
    """Reads local paths; a replay never writes."""

    def new_input(self, location: str) -> InputFile:
        return LocalInputFile(location.removeprefix("file://"))

    def new_output(self, location: str) -> OutputFile:
        raise PermissionError(f"replay does not write {location}")

    def delete(self, location: str | InputFile | OutputFile) -> None:
        raise PermissionError(f"replay does not delete {location}")


def fail(message: str) -> None:
    print(f"replay: {message}", file=sys.stderr)
    sys.exit(1)


def expected(history: dict) -> dict:
    """Each schema id's fields as (id, name, type, required) and its key."""
    return {
        s["schema-id"]: (
            [(f["id"], f["name"], f["type"], f["required"]) for f in s["fields"]],
            sorted(s.get("identifier-field-ids", [])),
        )
        for s in history["schemas"]
    }


def loaded(table: StaticTable) -> dict:
    """The same, as pyiceberg reads the table's metadata."""
    return {
        s.schema_id: (
            [(f.field_id, f.name, str(f.field_type), f.required) for f in s.fields],
            sorted(s.identifier_field_ids),
        )
        for s in table.metadata.schemas
    }


def run_of(snapshot) -> tuple:
    """What a snapshot says of its run."""
    s = snapshot.summary
    return (
        snapshot.schema_id,
        [s["utilake.report-type"], s["utilake.report-subtype"]],
        int(s["utilake.version"]),
        int(s["utilake.files"]),
        int(s["utilake.rows"]),
        s["utilake.from.file"],
        s["utilake.to.file"],
    )


def main() -> None:
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "target/iceberg")
    histories = Path(sys.argv[2] if len(sys.argv) > 2 else "evidence/histories")
    wanted = {p.stem: p for p in sorted(histories.glob("*.json"))}
    written = {p.name for p in out.iterdir() if p.is_dir()} if out.is_dir() else set()
    for name in sorted(written - wanted.keys()):
        fail(f"{out / name} has no history in {histories}")
    duck = duckdb.connect()
    duck.execute("INSTALL iceberg; LOAD iceberg;")
    snapshots = 0
    for name, path in wanted.items():
        history = json.loads(path.read_text())
        meta = out / name / "metadata"
        if not meta.is_dir():
            fail(f"{name}: {meta} was not written")
        version = (meta / "version-hint.text").read_text().strip()
        table = StaticTable.from_metadata(
            str((meta / f"v{version}.metadata.json").resolve()),
            properties={"py-io-impl": "__main__.LocalFileIO"},
        )
        want, have = expected(history), loaded(table)
        if want != have:
            fail(f"{name}: pyiceberg reads schemas {have}, the history has {want}")
        current = history.get("current-schema-id", 0)
        if table.metadata.current_schema_id != current:
            fail(
                f"{name}: the current schema is {table.metadata.current_schema_id}, "
                f"the history's is {current}"
            )
        if table.properties.get("utilake.table") != name:
            fail(f"{name}: the metadata names table {table.properties.get('utilake.table')!r}")
        # One snapshot per run, in order, saying what the run says.
        snaps = sorted(table.metadata.snapshots, key=lambda s: s.sequence_number)
        runs = [
            (
                r["schema-id"],
                r["report"],
                r["version"],
                r["files"],
                r["rows"],
                r["from"]["file"],
                r["to"]["file"],
            )
            for r in history["runs"]
        ]
        have_runs = [run_of(s) for s in snaps]
        if have_runs != runs:
            fail(f"{name}: the snapshots say {have_runs}, the history's runs are {runs}")
        times = [s.timestamp_ms for s in snaps]
        if times != sorted(set(times)):
            fail(f"{name}: snapshot times do not rise: {times}")
        current_snapshot = table.current_snapshot()
        if snaps and (current_snapshot is None or current_snapshot != snaps[-1]):
            fail(f"{name}: the current snapshot is not the last run's")
        if not snaps and current_snapshot is not None:
            fail(f"{name}: a table of no runs has a current snapshot")
        # The manifest lists open and hold nothing: a scan reads no rows.
        rows = table.scan().to_arrow()
        if rows.num_rows != 0:
            fail(f"{name}: a scan of a table without data read {rows.num_rows} rows")
        columns = [f.name for f in table.schema().fields]
        if rows.column_names != columns:
            fail(f"{name}: a scan reads columns {rows.column_names}")
        # DuckDB reads the same table.
        location = str((out / name).resolve()).replace("'", "''")
        (count,) = duck.execute(f"SELECT count(*) FROM iceberg_scan('{location}')").fetchone()
        if count != 0:
            fail(f"{name}: DuckDB read {count} rows")
        duck_columns = [
            c[0] for c in duck.execute(f"DESCRIBE SELECT * FROM iceberg_scan('{location}')").fetchall()
        ]
        if duck_columns != columns:
            fail(f"{name}: DuckDB reads columns {duck_columns}, the current schema has {columns}")
        (duck_snaps,) = duck.execute(
            f"SELECT count(*) FROM iceberg_snapshots('{location}')"
        ).fetchone()
        if duck_snaps != len(snaps):
            fail(f"{name}: DuckDB sees {duck_snaps} snapshots, the table has {len(snaps)}")
        snapshots += len(snaps)
    print(
        f"replay: {len(wanted)} table(s) read back as their histories say by pyiceberg "
        f"and DuckDB, {snapshots} snapshot(s) scanned empty"
    )


if __name__ == "__main__":
    main()
