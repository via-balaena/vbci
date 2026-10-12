# Data license

VBCI's own data records are dedicated to the public domain under CC0 1.0 Universal
([LICENSE-CC0](LICENSE-CC0), the legal code as Creative Commons publishes it). The code and its
documentation are licensed under MIT OR Apache-2.0 (README, "License"). Decided with Jon on
2026-10-11 (`docs/architecture.md`, "Plan").

## What is CC0

The records, in these paths. None exists yet; each arrives with the plan step that builds it.

- `sources.toml`
- `nutrients.toml`
- `ingredients/`
- `diet/`
- `units/`
- `recipes/`

A test in `crates/repo` holds this list to the records layer that `docs/architecture.md` describes.

## Contributions

GitHub's Terms of Service, section D.6, "Contributions Under Repository License" (read on
2026-10-11): "Whenever you add Content to a repository containing notice of a license, you license
that Content under the same terms, and you agree that you have the right to license that Content
under those terms." This file is the notice for the paths above.

## What is not ours

- **USDA FoodData Central** is not in the repo: `balaena fetch` downloads it (plan step 3), and
  `sources.toml` will record each dataset's terms. data.gov's record for FoodData Central gives its
  license as the U.S. Public Domain label, https://www.usa.gov/publicdomain/label/1.0/, with the
  Agricultural Research Service as publisher (record modified 2025-11-21; read 2026-10-11).
- **Data under another license** (ODbL, CC BY, MIT, AGPL) comes into a CC0 record only with its
  holder's permission. `docs/prior-art.md` lists the sources weighed and their licenses.
