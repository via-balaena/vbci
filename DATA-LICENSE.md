# Data license

VBCI's own data records are dedicated to the public domain under CC0 1.0 Universal
([LICENSE-CC0](LICENSE-CC0), the legal code as Creative Commons publishes it). The code and its
documentation are licensed under MIT OR Apache-2.0 (README, "License"). Decided with Jon on
2026-10-11 (`docs/architecture.md`, "Plan").

## What is CC0

The records, in these paths.

- `sources.toml`
- `nutrients.toml`
- `ingredients/`
- `diet/`
- `units/`
- `recipes/`

A test in `crates/repo` holds this list to the records layer that `docs/architecture.md` describes.

## The export

`export/datapackage.json` is generated from the records above, USDA's data and Wikidata's, all
CC0, and is CC0 too: its descriptor says so, and the warehouse does not write it while a publisher
in `sources.toml` licenses its data otherwise. It lists FoodData Central's datasets and the pinned
Wikidata revisions as its sources, with USDA's suggested citation; Wikidata asks for none.

## Contributions

GitHub's Terms of Service, section D.6, "Contributions Under Repository License" (read on
2026-10-11): "Whenever you add Content to a repository containing notice of a license, you license
that Content under the same terms, and you agree that you have the right to license that Content
under those terms." This file is the notice for the paths above.

## What is not ours

- **USDA FoodData Central**: its zips are not in the repo, as
  `cargo run -p balaena-warehouse -- fetch` downloads them into a cache, but the export holds values
  taken from them. USDA publishes FoodData Central under CC0 1.0 and asks to be listed as the
  source; `sources.toml` quotes its terms and its suggested citation from
  https://fdc.nal.usda.gov/api-guide.html (read 2026-10-11). data.gov's record for FoodData Central
  gives the U.S. Public Domain label (record modified 2025-11-21; read 2026-10-11).
- **Data under another license** (ODbL, CC BY, MIT, AGPL) comes into a CC0 record only with its
  holder's permission. `docs/prior-art.md` lists the sources weighed and their licenses.
