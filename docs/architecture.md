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
| Every dependency is pinned to one version: each crate's own with `=`, and the rest by `Cargo.lock`, which every cargo command in the Quality Gate but `cargo fmt` runs with `--locked`, so a missing or stale lockfile fails the gate. The compiler is pinned to one release by `rust-toolchain.toml`, and every action a workflow uses by commit. | `every_dependency_is_pinned` (what counts as pinned: `what_counts_as_pinned`), `the_gate_builds_with_the_lockfile` (what counts as a cargo command: `what_counts_as_a_cargo_command`), `the_toolchain_is_one_release`, `every_action_is_pinned_by_commit` |
| Code is formatted, clippy finds nothing at `-D warnings`, and the tests pass. | the Quality Gate's steps, in `.github/workflows/quality-gate.yml` |
| A newer push to a pull request cancels the Quality Gate's older run on it; a run on `main`, or one started by hand, is a concurrency group of its own, so no other run cancels it or holds it back. | `a_newer_push_cancels_only_a_pull_request_s_older_gate_run`, on the workflow's text. That GitHub runs it so is seen on a pull request, not tested here. |
| This doc names only tests and files that exist, up to its plan. | `the_architecture_doc_names_real_tests_and_files` |
| The project's own data records, in the paths `DATA-LICENSE.md` lists, are CC0, with the legal code in `LICENSE-CC0`; the code and its documentation are MIT OR Apache-2.0. | `the_data_license_covers_every_records_path` holds that list to the records row of the Layers table below, and `license_cc0_is_the_cc0_legal_code` holds the legal code to the text Creative Commons served. That every file in those paths is a record, and that no record lies outside them, is held by nothing: no record exists yet (`unknown`). |
| Every schema refuses a key it doesn't know. | `a_source_record_refuses_a_key_it_does_not_know` and `a_nutrient_record_refuses_a_key_it_does_not_know`, for `sources.toml` and `nutrients.toml`, the records so far |
| `sources.toml` pins each dataset by its zip's sha256, as 64 lowercase hex digits, and its length; a dataset has an id no other has, names a publisher the record lists, and is fetched over https; a publisher gives its terms, the https page they come from, and the day they were read. | `a_dataset_is_pinned_by_a_lowercase_sha256_and_a_length`, `dataset_ids_are_unique_and_plain`, `a_dataset_names_a_publisher_the_record_lists`, `a_dataset_and_its_terms_are_fetched_over_https`, `a_publisher_says_when_its_terms_were_read`, `the_sources_record_loads` |
| A zip is used only as pinned: fetching refuses a download whose length or sha256 differs and writes nothing, refuses a cached zip that differs and leaves it as it is, and keeps a good zip in the cache under its sha256; loading refuses a cached zip that differs; extraction refuses a file whose path would climb out of its folder. | `bytes_that_differ_from_the_pin_are_refused`, `a_cached_zip_that_differs_from_its_pin_is_refused`, `a_good_download_is_cached_under_its_sha256`, `loading_refuses_a_cached_zip_that_differs`, `a_member_that_would_extract_outside_its_folder_is_refused`. A download is abandoned after 300 s (`DOWNLOAD_TIMEOUT`); nothing tests that, as it needs the network. The cache is `$VBCI_CACHE`, else `$XDG_CACHE_HOME/vbci`, else `$HOME/.cache/vbci`: `the_cache_follows_the_environment`. |
| Bronze holds `sources.toml` as read, every file in each pinned zip with its length and sha256, and every CSV in a zip row for row, all text; each bronze USDA row names the zip and the file it came from. A CSV whose name would not make a plain table name is refused, not renamed. | `the_sources_record_is_in_bronze_as_read`; `every_file_in_the_pinned_zips_is_in_bronze_with_its_sha256`; `every_csv_in_the_pinned_zips_is_in_bronze_row_for_row`, which counts each CSV's records apart from DuckDB (`what_counts_as_a_csv_record`); `every_usda_row_traces_to_the_bytes_it_came_from`; `what_counts_as_a_csv_table` |
| SR Legacy's bronze row counts equal USDA's own count of them, `all_downloaded_table_record_counts.csv`. | `the_sr_legacy_row_counts_equal_usda_s_own` |
| `nutrients.toml` gives each nutrient a plain key no other has, a name, the unit USDA writes, and its USDA codes as text, digits or digits, a point and digits, each code in one nutrient only. | `a_code_is_written_as_usda_writes_it`, `a_code_belongs_to_one_nutrient`, `nutrient_keys_are_unique_and_plain`, `the_nutrients_record_loads` |
| Silver holds every SR Legacy and Foundation food, meat included, and each of their nutrient rows is kept or rejected with its reason, never both: no amount; a nutrient its dataset's `nutrient.csv` lacks; a repeat of an earlier row for the same food and nutrient with the same amount (the earliest by id is kept); or a disagreement with another such row (all are rejected, so the food has no value rather than a guess). | `silver_holds_every_sr_legacy_and_foundation_food`, `every_food_nutrient_row_in_scope_is_kept_or_rejected`, and on rows written for the purpose, `silver_keeps_a_row_or_rejects_it_with_its_reason` |
| Silver's keys refuse a second food with one fdc_id, a food in no category of its dataset or of another data type, an amount for a nutrient its dataset lacks, a second amount for one food and nutrient, a code in two nutrients, and a rejection with no reason. | `the_database_refuses_what_silver_s_keys_refuse`, with an accepted control beside each |
| A nutrient's codes are matched exactly as written, never rounded, and a food's value comes from the first code it has a value for. A code some dataset's `nutrient.csv` lacks, or has with another unit, stops the load. | `codes_are_matched_exactly_not_rounded`, `the_first_listed_code_with_a_value_is_used`, `a_nutrient_code_usda_does_not_have_is_refused`, `a_nutrient_whose_unit_differs_is_refused` |
| Every silver row names a dataset in `bronze.usda_zip` or a record in `bronze.record`, and each kept nutrient row the bronze row it came from. | `every_silver_row_traces_to_the_bytes_it_came_from`: DuckDB refuses foreign keys across schemas, so a test holds this link |
| `docs/coverage.md` and `docs/data-catalogue.md` are what the warehouse writes, and every table and view says what it is. | `the_coverage_report_is_current`, `the_catalogue_is_current`, `every_table_and_view_says_what_it_is` |
| Parsing records needs no database and no network. | `parsing_records_needs_no_database_or_network` |
| `main` changes only through a pull request that passes the Quality Gate on a branch up to date with `main`, with signed commits and linear history, and is never force-pushed or deleted. | the `main-protection` ruleset on GitHub, which lists no one who may bypass it, as its settings read on 2026-10-11. Nothing here tests the ruleset. |

The tests that span crates live in `crates/repo`, a member of the workspace in `Cargo.toml`. The
records' schemas are in `crates/records`, and the warehouse is `crates/warehouse`, with its layers
in `crates/warehouse/schema.sql`. Its tests read USDA's zips from the cache: fill it once with
`cargo run -p balaena-warehouse -- fetch`, as the Quality Gate does before its tests.

## Plan

### Decided with Jon, 2026-10-11

1. **The project's own data records are CC0** (ingredients, aliases, diet exceptions, recipes, and
   later farm reference data); the code stays MIT OR Apache-2.0. `DATA-LICENSE.md` lands in step 2.
   The cost: seed data under another license (open-tandoor-data's ODbL mappings, capycook's MIT
   ones) can come in only with its holder's permission.
2. **This plan,** reviewed on 2026-10-11 against USDA's downloads (SR Legacy 2018-04 and Foundation
   2025-04-24, 2025-12-18 and 2026-04-30), cooklang 0.19.0's source, and three prior-art searches.
   Facts below marked "scratch check" were seen then with throwaway scripts that are not in the
   repo, so they are `unknown` until a test holds them.

### Layers

| layer | where | what it is |
|---|---|---|
| records | `sources.toml`, `nutrients.toml`, `ingredients/`, `diet/`, `units/`, `recipes/<name>.cook` | The system of record, reviewed by pull request. |
| upstream | USDA's zips, fetched by `cargo run -p balaena-warehouse -- fetch` into the user's cache, never committed | Pinned in `sources.toml` by sha256 and length, with URL and release, and the publisher's license, quoted terms and citation. |
| schemas and loaders | `crates/records` | Typed schemas that refuse a key they don't know. cooklang parses recipes here. |
| warehouse | `crates/warehouse`, DuckDB, described in a generated `docs/data-catalogue.md` | Bronze holds every file as read, with its sha256; silver holds typed rows with keys; gold holds views that answer questions. It can be deleted at any time. |
| export | one file generated from gold | What runtime readers load, stamped with the sha256s it came from. |
| readers | `crates/core` (analysis), `crates/balaena` (the CLI), later the app | They read the export: a recipe a user writes is not a record, so it does not pass through the warehouse, and DuckDB never ships in the CLI. |

### Steps

Each step is one pull request, done when its check passes.

1. **Skeleton.** The workspace, `rust-toolchain.toml`, `crates/repo` with the pinning tests, the
   Quality Gate's fmt, clippy and test steps, and this doc held to the code. Done:
   `every_dependency_is_pinned`, `the_gate_builds_with_the_lockfile`,
   `what_counts_as_a_cargo_command`, `the_toolchain_is_one_release`,
   `every_action_is_pinned_by_commit`, `the_architecture_doc_names_real_tests_and_files`.
2. **Prior art and the data license.** `docs/prior-art.md`, dated, from the 2026-10-11 searches
   (identifiers for foods and nutrients; tools that already link recipes to USDA data; farm
   software), and `DATA-LICENSE.md` with CC0's legal code in `LICENSE-CC0`. Done: each step below
   names what it reuses, `the_data_license_covers_every_records_path` and
   `license_cc0_is_the_cc0_legal_code`.
3. **USDA data, pinned.** `sources.toml` names SR Legacy (2018-04) and Foundation (2026-04-30, the
   newest release USDA lists) by sha256 and length, with USDA's terms quoted from its API guide:
   CC0. `cargo run -p balaena-warehouse -- fetch` refuses bytes whose sha256 differs: a URL is not
   an identity (scratch check: the "2018-04" SR Legacy zip is served with a Last-Modified of
   2023-10-25). Bronze holds each CSV as read, all text, with its file's sha256; CI fetches through
   a cache. Reuses DuckDB's `read_csv`, ureq for https, the zip crate, and sha2. Done:
   `bytes_that_differ_from_the_pin_are_refused`, `the_sr_legacy_row_counts_equal_usda_s_own`, and
   the rules above. GitHub's runners reach fdc.nal.usda.gov: this step's first run on GitHub (run
   38190614286) downloaded both zips, as pinned, before its tests.
4. **USDA's tables in silver, and our nutrients.** Silver types every SR Legacy and Foundation food
   (data types `sr_legacy_food` and `foundation_food`; the Foundation zip's sample and acquisition
   rows stay in bronze), meat included, with its category, and every nutrient row of those foods, or
   rejects a row with its reason. Foods are keyed by fdc_id (scratch check: the two datasets share
   none, and across three Foundation releases no fdc_id was dropped and none changed its
   description). Categories and nutrients are keyed by dataset (scratch check: 10 nutrient rows with
   the same id differ between SR Legacy and Foundation). `nutrients.toml` gives each of 16 nutrients
   its USDA codes, exactly and in priority order (sugars: 269, then 269.3), and the unit each must
   have; `gold.food_nutrient` gives each food's value per 100 g, and `docs/coverage.md` and
   `docs/data-catalogue.md` are generated. Found on loading: Foundation 2026-04-30 has 33 nutrient
   rows with no amount, for a nutrient its `nutrient.csv` lacks, and one row repeated; both are in
   `docs/coverage.md`'s rejected rows. Reuses no FDC reader, as the search found none;
   usda_fooddata's `schema.sql` (Apache-2.0, FDC's 2020-10 release) types these columns as silver
   does, as integers and text with a floating-point amount (REAL there, DOUBLE here), except
   `nutrient_nbr`, which it reads as REAL and silver keeps as text. Done:
   `codes_are_matched_exactly_not_rounded`, `a_nutrient_whose_unit_differs_is_refused`,
   `the_database_refuses_what_silver_s_keys_refuse`, `the_coverage_report_is_current`, and the rules
   above. USDA's portions and measure units come with step 7.
5. **Diet status per food,** vegan and vegetarian, derived and never typed: for Foundation foods
   from the source organism USDA records (NCBI taxon: plants, fungi or animals) and the category;
   for SR Legacy from the category; a category with mixed foods gives `unknown`. A reviewed
   exception in `diet/`, keyed by fdc_id, overrides. Done when the fixtures hold: oyster
   mushrooms, salsify and scallop squash are vegan; duck and turkey eggs vegetarian, not vegan;
   fish oils not vegetarian. Reuses USDA's organism attributes; cook.md's diet-badges (MIT) and Open
   Food Facts' flags are references only.
6. **Ingredient identity and aliases.** Our own ingredient ids in `ingredients/`, each linked per
   state (raw, cooked, canned, dry) to an fdc_id in the pinned release, with FoodOn ids and Wikidata
   ids as cross-references (scratch check: USDA's Foundation 2026-04-30 files give a FoodOn id for
   365 of its 469 foods). Alias candidates come from sources free to reuse: SR Legacy's common
   names, Wikidata and OpenCookbook, all CC0; each is `unknown` until reviewed. capycook's and
   open-tandoor-data's mappings come in only with their holders' permission. Done when an alias
   naming no ingredient, or a link to a food outside the pinned release, is refused.
7. **Units and portions.** cooklang's bundled units for mass and volume, their spellings and
   `fl oz`, and count units (clove, bunch, head) as records in `units/`. cooklang 0.19.0 builds only
   with its `aisle` feature on (its `ingredient_list.rs` imports `aisle` whatever the features), so
   that is on too. USDA's portion text is read by the same converter as recipes. Done when every
   portion converts or is listed in a generated report.
8. **The export.** Gold written to one file that `crates/core` reads, with the sha256s it came from.
   Done when a test holds it to what the warehouse writes. What it reuses for the file's format is
   searched in this step.
9. **Analysis and recipe reports.** `crates/core` turns a recipe and the export into grams,
   nutrients and diet status. How sure each number is comes from how it was reached, never typed,
   and a total says how many lines had data. Done when the reports for `recipes/<name>.cook` are
   generated and held current; the red lentil soup is the first. Reuses cooklang's parser.
10. **The CLI:** `balaena analyze`, `balaena search` and `balaena info`, reading only the export.
    What it reuses for parsing arguments is searched in this step.

**Later:** farm reference data. Crops link to ingredients through the organism, as Growstuff does
with GBIF and Wikidata ids and AGROVOC with GBIF. A grower's weight per bunch or head comes from
harvests that record both a count and a weight, so it is measured, not typed; the 2026-10-11 search
found none stored in farmOS, Open Food Network, LiteFarm, FarmBot or Growstuff. Interchange through
farmOS's JSON:API and the Data Food Consortium's, which Open Food Network uses. Whether aquaponic
fish are ever an in-app product is decided then.

### Grades

Tentzhen's grades, where they apply: a USDA value is `trusted`, an entry per pinned dataset in a
trusted base; a farm's own lab result is `measured`; a fuzzy match or a portion borrowed from
another food is `unknown` until someone reviews it; a derived number takes the weakest grade of
what it rests on. The exact mapping is settled in steps 6 and 9.

### Open

- Whether bare FoodOn ids may sit in CC0 records: `unknown`.
- The license of the Open Food Facts ingredients taxonomy: `unknown`.
- USDA asks to be told about a product that uses FoodData Central's data (its terms, quoted in
  `sources.toml`): a step on Jon's account, once VBCI ships one.
- A crate published for others to depend on would pass its `=` pins on to them. How to pin one is
  decided if a crate is ever published.
