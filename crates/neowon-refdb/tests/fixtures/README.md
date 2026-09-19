# Importer fixtures

Small, hand-written excerpts of each source's format, one per importer.
They exist to pin the parsers; no network runs in any test (D19).

| File | Source and licence |
|---|---|
| `wikidata.json` | Wikidata SPARQL JSON (`application/sparql-results+json`), CC0. Shapes match WDQS output; the entities are invented. |
| `eibi.csv` | EiBi shortwave schedule CSV, **Latin-1**, `;`-separated. EiBi data: Eike Bierwirth, see eibispace.de for terms. |
| `ourairports-*.csv` | OurAirports data dump (`airport-frequencies.csv`, `airports.csv`), public domain. Rows are invented. |
| `fcc.txt` | FCC `fmq`/`amq` pipe-delimited output (`list=4`), US public domain. Rows **captured from the live service 2026-09-19** (headerless format, DMS in separate columns) plus one malformed row. |
| `fmlist.csv` | FMLIST export, operator's own. FMLIST terms apply. Rows are invented; the real header shape is recorded in the spec's Deviations when the operator supplies a file. |

Every fixture stays under 50 rows. `tests/importers.rs` asserts golden counts
and field-exact rows, then truncates each fixture at every line boundary and
re-parses it: a malformed row must become a report entry, never a panic.
