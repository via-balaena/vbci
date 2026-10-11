# Architecture

How VBCI's records, code and checks fit together, the rules that keep them honest, and the plan
for what comes next. Every rule below names what holds it, and a rule nothing holds says so. The
design follows [tentzhen's](https://github.com/via-balaena/tentzhen/blob/main/docs/architecture.md):
records are the system of record, typed loaders refuse what they don't know, a warehouse is rebuilt
from them, and readers read only its last layer.

## Where it is headed

Jon, 2026-10-11: at its peak, a farm/greenhouse/hydroponic/aquaponic farm-to-table vegan/vegetarian
app, and "a very data engineering oriented app". The foundation comes first: reference data about
foods, ingredients, recipes and diets, rebuilt from pinned sources and reviewed records.

## Two kinds of data

- **Reference data:** USDA FoodData Central, and the project's own ingredients, aliases, diet
  exceptions and recipes. Public, slow to change, reviewed by pull request: the repo is its system
  of record, and the plan below builds it.
- **Operational data:** a farm's harvests, inventory and orders. Private to each farm and changing
  daily, it does not belong in a public repo. It gets its own design when the app does, and reads
  the reference data.

## Rules

| rule | held by |
|---|---|
| Every dependency is pinned to one version: each crate's own with `=`, and the rest by `Cargo.lock`, which the Quality Gate builds with `--locked`. The compiler is pinned to one release by `rust-toolchain.toml`, and every action a workflow uses by commit. | `every_dependency_is_pinned` (what counts as pinned: `what_counts_as_pinned`), `the_gate_builds_with_the_lockfile`, `the_toolchain_is_one_release`, `every_action_is_pinned_by_commit` |
| Code is formatted, clippy finds nothing at `-D warnings`, and the tests pass. | the Quality Gate's steps, in `.github/workflows/quality-gate.yml` |
| A newer push to a pull request cancels the Quality Gate's older run on it; a run on `main`, or one started by hand, is never cancelled. | `a_newer_push_cancels_only_a_pull_request_s_older_gate_run`, on the workflow's text. That GitHub runs it so is seen on a pull request, not tested here. |
| This doc names only tests and files that exist, up to its plan. | `the_architecture_doc_names_real_tests_and_files` |
| `main` changes only through a pull request that passes the Quality Gate on a branch up to date with `main`, with signed commits and linear history, and is never force-pushed or deleted. | the `main-protection` ruleset on GitHub, as its settings read on 2026-10-11. Nothing here tests the ruleset. |

The tests that span crates live in `crates/repo`, a member of the workspace in `Cargo.toml`.

## Plan

### Decided with Jon, 2026-10-11

1. **The project's own data records are CC0** (ingredients, aliases, diet exceptions, recipes, and
   later farm reference data); the code stays MIT OR Apache-2.0. `DATA-LICENSE.md` lands in step 2.
   The cost: seed data under another license (open-tandoor-data's ODbL mappings, capycook's MIT
   ones) can come in only with its holder's permission.
2. **This plan,** reviewed on 2026-10-11 against USDA's downloads (SR Legacy 2018-04 and Foundation
   2025-04-24, 2025-12-18 and 2026-04-30), cooklang 0.19.0's source, and three prior-art searches.
   Facts below marked "scratch check" were seen then with throwaway scripts; the step that relies on
   one holds it by a test.

### Layers

| layer | where | what it is |
|---|---|---|
| records | `sources.toml`, `nutrients.toml`, `ingredients/`, `diet/`, `units/`, `recipes/<name>.cook` | The system of record, reviewed by pull request. |
| upstream | USDA's zips, fetched by `balaena fetch` into the user's cache, never committed | Pinned in `sources.toml` by sha256, with URL, release, license and citation. |
| schemas and loaders | `crates/records` | Typed schemas that refuse a key they don't know. cooklang parses recipes here. |
| warehouse | `crates/warehouse`, DuckDB, described in a generated `docs/data-catalogue.md` | Bronze holds every file as read, with its sha256; silver holds typed rows with keys; gold holds views that answer questions. It can be deleted at any time. |
| export | one file generated from gold | What runtime readers load, stamped with the sha256s it came from. |
| readers | `crates/core` (analysis), `crates/balaena` (the CLI), later the app | They read the export: a recipe a user writes is not a record, so it cannot pass through the warehouse, and DuckDB never ships in the CLI. |

### Steps

Each step is one pull request, done when its check passes.

1. **Skeleton.** The workspace, `rust-toolchain.toml`, `crates/repo` with the pinning tests, the
   Quality Gate's fmt, clippy and test steps, and this doc held to the code. Done:
   `every_dependency_is_pinned`, `the_gate_builds_with_the_lockfile`,
   `the_toolchain_is_one_release`, `every_action_is_pinned_by_commit`,
   `the_architecture_doc_names_real_tests_and_files`.
2. **Prior art and the data license.** `docs/prior-art.md`, dated, from the 2026-10-11 searches
   (identifiers for foods and nutrients; tools that already link recipes to USDA data; farm
   software), and `DATA-LICENSE.md`. Done when each step below names what it reuses.
3. **USDA data, pinned.** `sources.toml` names SR Legacy (2018-04) and Foundation (2026-04-30, the
   newest release USDA lists) by sha256. `balaena fetch` refuses bytes whose sha256 differs: a URL is
   not an identity (scratch check: the "2018-04" SR Legacy zip is served with a Last-Modified of
   2023-10-25). Bronze holds each CSV as read, all text, with its file's sha256; CI fetches through a
   cache keyed by the sha256s. Done when mismatched bytes are refused, and SR Legacy's row counts
   equal USDA's own `all_downloaded_table_record_counts.csv`. Whether GitHub's runners reach
   fdc.nal.usda.gov is `unknown` until this step's first run; the fallback, a mirror on a GitHub
   release, is a step on Jon's account.
4. **USDA's tables in silver, and our nutrients.** Every food of both datasets, meat included:
   choosing what a reader sees is gold's job, and "chicken stock" should match chicken. Foods are
   keyed by fdc_id (scratch check: the two datasets share none, and no Foundation fdc_id was dropped
   or reused across three releases). Nutrient, unit and category tables are keyed by release
   (scratch check: 10 nutrient rows with the same id differ between SR Legacy and Foundation).
   `nutrients.toml` gives each of our nutrients its USDA codes, exactly and in priority order
   (sugars: 269, then 269.3), and the unit each must have. Gold gives each food's nutrients per
   100 g, and `docs/coverage.md` is generated from it. Done when a code rounded or changed, a wrong
   unit or a duplicate key is refused, and `docs/coverage.md` is current.
5. **Diet status per food,** vegan and vegetarian, derived and never typed: for Foundation foods
   from the source organism USDA records (NCBI taxon: plants, fungi or animals) and the category;
   for SR Legacy from the category; a category with mixed foods gives `unknown`. A reviewed
   exception in `diet/`, keyed by fdc_id, overrides. Done when the fixtures hold: oyster
   mushrooms, salsify and scallop squash are vegan; duck and turkey eggs vegetarian, not vegan;
   fish oils not vegetarian.
6. **Ingredient identity and aliases.** Our own ingredient ids in `ingredients/`, each linked per
   state (raw, cooked, canned, dry) to an fdc_id in the pinned release, with FoodOn ids (USDA's
   Foundation files carry them) and Wikidata ids as cross-references. Alias candidates come from CC0
   sources (SR Legacy's common names, Wikidata, OpenCookbook), each `unknown` until reviewed. Done
   when an alias naming no ingredient, or a link to a food outside the pinned release, is refused.
7. **Units and portions.** cooklang's bundled units for mass and volume, their spellings and
   `fl oz` (cooklang 0.19.0 compiles only with its `aisle` feature on, so that is on too), and count
   units (clove, bunch, head) as records in `units/`. USDA's portion text is read by the same
   converter as recipes. Done when every portion converts or is listed in a generated report.
8. **The export.** Gold written to one file that `crates/core` reads, with the sha256s it came from.
   Done when a test holds it to what the warehouse writes.
9. **Analysis and recipe reports.** `crates/core` turns a recipe and the export into grams,
   nutrients and diet status. How sure each number is comes from how it was reached, never typed,
   and a total says how many lines had data. Done when the reports for `recipes/<name>.cook` are
   generated and held current; the red lentil soup is the first.
10. **The CLI:** `balaena analyze`, `balaena search` and `balaena info`, reading only the export.

**Later:** farm reference data. Crops link to ingredients through the organism, as Growstuff and
AGROVOC do with GBIF and Wikidata ids. A grower's weight per bunch or head comes from harvests that
record both a count and a weight, so it is measured, not typed; none of the farm software the
2026-10-11 search found stores one. Interchange through farmOS's JSON:API and the Data Food
Consortium's, which Open Food Network uses. Whether aquaponic fish are ever an in-app product is
decided then.

### Grades

Tentzhen's grades, where they apply: a USDA value is `trusted`, an entry per pinned dataset in a
trusted base; a farm's own lab result is `measured`; a fuzzy match or a portion borrowed from
another food is `unknown` until someone reviews it; a derived number takes the weakest grade of
what it rests on. The exact mapping is settled in steps 6 and 9.

### Open

- Whether bare FoodOn ids may sit in CC0 records: `unknown`.
- The license of the Open Food Facts ingredients taxonomy: `unknown`.
- How long DuckDB takes to build in this repo's CI: `unknown`. Tentzhen's gate allows 40 minutes
  for a cold build, by its workflow's comment.
- A crate published for others to depend on would need version ranges, which
  `every_dependency_is_pinned` refuses. Decided if a crate is ever published.
