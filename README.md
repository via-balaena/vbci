# VBCI

The Via Balaena Culinary Institute. At its peak, a farm/greenhouse/hydroponic/aquaponic
farm-to-table vegan/vegetarian app, and "a very data engineering oriented app" (Jon, 2026-10-11).
How it is built, the rules that keep it honest, and the plan: `docs/architecture.md`.

`main` changes only through a pull request, with signed commits and linear history.
A pull request must pass the Quality Gate (`.github/workflows/quality-gate.yml`): formatting,
clippy, and the tests, including the repo's own checks in `crates/repo`. The warehouse's tests read
USDA's data from a cache: fill it once with `cargo run -p balaena-warehouse -- fetch` (into
`$VBCI_CACHE`, else `$XDG_CACHE_HOME/vbci`, else `~/.cache/vbci`), which downloads the zips
`sources.toml` pins and refuses any whose sha256 differs.

## Use

`balaena`, the command line, reads only the export, which is built into it:

```
cargo run -p balaena -- analyze recipes/red-lentil-soup.cook
cargo run -p balaena -- search red lentil
cargo run -p balaena -- info
```

`analyze` prints a recipe's report, as `docs/recipes/` holds VBCI's own; `search` finds
ingredients and USDA foods by name, for writing an ingredient's aliases and forms; `info` says what
the export came from.

## License

The code and its documentation are licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

The project's own data records, in the paths [DATA-LICENSE.md](DATA-LICENSE.md) lists, are
dedicated to the public domain under CC0 1.0 Universal ([LICENSE-CC0](LICENSE-CC0)).
So is the export, `export/datapackage.json`, generated from them and USDA's data.
