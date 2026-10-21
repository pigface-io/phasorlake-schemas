"""Read every table `nem-schemas iceberg` wrote with pyiceberg, and hold it
to its history.

    uv run --with pyiceberg python scripts/replay.py [DIR] [HISTORIES]

DIR (default target/iceberg) holds one table per directory, as
`nem-schemas iceberg --out DIR` writes them; HISTORIES (default
evidence/histories) holds each table's history. Every history must have a
table and every table a history. Each table is loaded as a pyiceberg
`StaticTable` from the metadata its `version-hint.text` names, and must
have the history's schemas by id, each with its fields' ids, names and
nullability in order and its key, and the schema of the last run as its
current schema. Exits 1 on the first table that does not, naming it.

The tables hold no data, so the files are read with a FileIO of this
script's own (`LocalFileIO`): no pyarrow or fsspec is needed.
"""

import json
import sys
from pathlib import Path

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
    """Each schema id's fields as (id, name, required) and its key."""
    return {
        s["schema-id"]: (
            [(f["id"], f["name"], f["required"]) for f in s["fields"]],
            sorted(s.get("identifier-field-ids", [])),
        )
        for s in history["schemas"]
    }


def loaded(table: StaticTable) -> dict:
    """The same, as pyiceberg reads the table's metadata."""
    return {
        s.schema_id: (
            [(f.field_id, f.name, f.required) for f in s.fields],
            sorted(s.identifier_field_ids),
        )
        for s in table.metadata.schemas
    }


def main() -> None:
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "target/iceberg")
    histories = Path(sys.argv[2] if len(sys.argv) > 2 else "evidence/histories")
    wanted = {p.stem: p for p in sorted(histories.glob("*.json"))}
    written = {p.name for p in out.iterdir() if p.is_dir()} if out.is_dir() else set()
    for name in sorted(written - wanted.keys()):
        fail(f"{out / name} has no history in {histories}")
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
        runs = history["runs"]
        current = runs[-1]["schema-id"] if runs else min(want, default=0)
        if table.metadata.current_schema_id != current:
            fail(
                f"{name}: the current schema is {table.metadata.current_schema_id}, "
                f"the last run's is {current}"
            )
    print(f"replay: {len(wanted)} table(s) read back as their histories say")


if __name__ == "__main__":
    main()
