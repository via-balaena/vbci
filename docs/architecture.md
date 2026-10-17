# Architecture

How VBCI's records, code and checks fit together, the rules that keep them honest, and the plan
for what comes next. Every rule below names what holds it, and a rule nothing holds says so. The
design follows [tentzhen's](https://github.com/via-balaena/tentzhen/blob/main/docs/architecture.md):
records are the system of record, typed loaders refuse what they don't know, a warehouse is rebuilt
from them, and readers read only its last layer.

## Where it is headed

Jon, 2026-10-11: at its peak, a farm/greenhouse/hydroponic/aeroponic farm-to-table vegan/vegetarian
app, and "a very data engineering oriented app". (Jon wrote "aquaponic" and corrected it to
"aeroponic" on 2026-10-16.) The foundation comes first: reference data about foods, ingredients,
recipes and diets, rebuilt from pinned sources and reviewed records.

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
| The Quality Gate's fetch step runs the binary built by the cargo command that builds its tests, so the tests and the fetch step share one build of DuckDB, and clippy needs one more. | `the_gate_fetches_with_the_binary_its_tests_were_built_with`, on the workflow's text. How many times DuckDB builds is seen in a run's log, not tested here. |
| The Quality Gate's build cache (`target` and cargo's downloads) is saved and restored only under its exact key, a hash of `rust-toolchain.toml`, every `Cargo.toml`, `Cargo.lock` and the gate's workflow. | `the_build_cache_is_restored_only_by_its_exact_key`, on the workflow's text. What GitHub restores is seen in a run's log, not tested here. |
| A newer push to a pull request cancels the Quality Gate's older run on it; a run on `main`, or one started by hand, is a concurrency group of its own, so no other run cancels it or holds it back. | `a_newer_push_cancels_only_a_pull_request_s_older_gate_run`, on the workflow's text. That GitHub runs it so is seen on a pull request, not tested here. |
| This doc names only tests and files that exist, up to its plan. | `the_architecture_doc_names_real_tests_and_files` |
| The project's own data records, in the paths `DATA-LICENSE.md` lists, are CC0, with the legal code in `LICENSE-CC0`; the code and its documentation are MIT OR Apache-2.0. | `the_data_license_covers_every_records_path` holds that list to the records row of the Layers table below, and `license_cc0_is_the_cc0_legal_code` holds the legal code to the text Creative Commons served. That every file in `diet/`, `ingredients/`, `crops/`, `units/` and `recipes/` is a record is held by each folder's load (`the_diet_folder_holds_its_four_records_and_no_other_file`, `an_ingredient_is_named_by_its_file`, `a_crop_is_named_by_its_file`, `the_units_folder_holds_count_toml_alone`, `a_recipe_record_parses_without_warnings`). That no record the warehouse loads lies outside the listed paths is held by `every_record_the_warehouse_loads_is_under_the_data_license`; for the recipes, which `crates/core` reads, by nothing (`unknown`). |
| Every schema refuses a key it doesn't know. | `a_source_record_refuses_a_key_it_does_not_know`, `a_nutrient_record_refuses_a_key_it_does_not_know`, `a_diet_record_refuses_a_key_or_value_it_does_not_know`, `an_ingredient_file_refuses_a_key_or_value_it_does_not_know` (with `an_ingredient_names_its_organism_or_why_it_has_none` and `an_ingredient_says_what_it_is_made_from` for its `[organism]` and `[origin]` tables), `a_crop_file_refuses_a_key_or_value_it_does_not_know` and `a_count_unit_record_refuses_what_it_does_not_know`, for `sources.toml`, `nutrients.toml`, the four files of `diet/`, each file of `ingredients/` and `crops/` and `units/count.toml`, the records so far; and `an_export_is_a_v2_data_package_with_keys_it_knows`, for `export/datapackage.json` as `crates/core` reads it |
| `sources.toml` pins each dataset by its zip's sha256, as 64 lowercase hex digits, and its length; a dataset has an id no other has, names a publisher the record lists, and is fetched over https. It pins each Wikidata item, `Q<digits>` and listed once, by one revision's id, above 0, and the sha256 and length of the content Wikidata stores for that revision; the items come under `[publisher.wikidata]`. A publisher gives its terms, the https page they come from, the day they were read, and the citation it asks for, or none where its terms ask for none (Wikidata's licensing page, read 2026-10-16, asks for none). | `a_dataset_is_pinned_by_a_lowercase_sha256_and_a_length`, `dataset_ids_are_unique_and_plain`, `a_dataset_names_a_publisher_the_record_lists`, `a_dataset_and_its_terms_are_fetched_over_https`, `a_publisher_says_when_its_terms_were_read`, `a_wikidata_item_is_pinned_by_its_revision_s_sha256_and_length`, `a_wikidata_item_is_listed_once_under_wikidata_s_publisher`, `a_publisher_s_citation_is_one_it_asks_for`, `the_sources_record_loads` |
| A zip is used only as pinned: fetching refuses a download whose length or sha256 differs and writes nothing, refuses a cached zip that differs and leaves it as it is, and keeps a good zip in the cache under its sha256; loading refuses a cached zip that differs; extraction refuses a file whose path would climb out of its folder. | `bytes_that_differ_from_the_pin_are_refused`, `a_cached_zip_that_differs_from_its_pin_is_refused`, `a_good_download_is_cached_under_its_sha256`, `loading_refuses_a_cached_zip_that_differs`, `a_member_that_would_extract_outside_its_folder_is_refused`. A download is abandoned after 300 s (`DOWNLOAD_TIMEOUT`); nothing tests that, as it needs the network. The cache is `$VBCI_CACHE`, else `$XDG_CACHE_HOME/vbci`, else `$HOME/.cache/vbci`: `the_cache_follows_the_environment`. |
| A Wikidata revision is used only as pinned: fetching reads it from Wikidata's revisions API, refuses an answer about another item or revision, refuses content whose length or sha256 differs and writes nothing, refuses cached content that differs and leaves it as it is, and keeps good content in the cache under its item, revision and sha256, so a pin with any of the three changed is downloaded and checked again; loading refuses cached content that differs. Every download sends a User-Agent with contact details (`USER_AGENT`), as Wikimedia's User-Agent policy asks; nothing tests that, as it needs the network. | `an_answer_about_another_item_or_revision_is_refused`, `a_wikidata_revision_is_cached_under_its_pin_and_refused_where_it_differs`, `loading_refuses_cached_wikidata_content_that_differs` |
| Bronze holds `sources.toml` as read, every file in each pinned zip with its length and sha256, every CSV in a zip row for row, all text, and each pinned Wikidata revision's content with its pin; each bronze USDA row names the zip and the file it came from. A CSV whose name would not make a plain table name is refused, not renamed. | `the_sources_record_is_in_bronze_as_read`; `every_file_in_the_pinned_zips_is_in_bronze_with_its_sha256`; `every_csv_in_the_pinned_zips_is_in_bronze_row_for_row`, which counts each CSV's records apart from DuckDB (`what_counts_as_a_csv_record`); `every_pinned_wikidata_revision_is_in_bronze_as_pinned`; `every_usda_row_traces_to_the_bytes_it_came_from`; `what_counts_as_a_csv_table` |
| SR Legacy's bronze row counts equal USDA's own count of them, `all_downloaded_table_record_counts.csv`. | `the_sr_legacy_row_counts_equal_usda_s_own` |
| `nutrients.toml` gives each nutrient a plain key no other has, a name, the unit USDA writes, and its USDA codes as text, digits or digits, a point and digits, each code in one nutrient only. | `a_code_is_written_as_usda_writes_it`, `a_code_belongs_to_one_nutrient`, `nutrient_keys_are_unique_and_plain`, `the_nutrients_record_loads` |
| Silver holds every SR Legacy and Foundation food, meat included, and each of their nutrient rows is kept or rejected with its reason, never both: no amount; a nutrient its dataset's `nutrient.csv` lacks; a repeat of an earlier row for the same food and nutrient with the same amount (the earliest by id is kept); or a disagreement with another such row (all are rejected, so the food has no value rather than a guess). | `silver_holds_every_sr_legacy_and_foundation_food`, `every_food_nutrient_row_in_scope_is_kept_or_rejected`, and on rows written for the purpose, `silver_keeps_a_row_or_rejects_it_with_its_reason` |
| Silver's keys refuse a second food with one fdc_id, a food in no category of its dataset or of another data type, an amount for a nutrient its dataset lacks, a second amount for one food and nutrient, a code in two nutrients, and a rejection with no reason. | `the_database_refuses_what_silver_s_keys_refuse`, with an accepted control beside each |
| A nutrient's codes are matched exactly as written, never rounded, and a food's value comes from the first code it has a value for. A code some dataset's `nutrient.csv` lacks, or has with another unit, stops the load. | `codes_are_matched_exactly_not_rounded`, `the_first_listed_code_with_a_value_is_used`, `a_nutrient_code_usda_does_not_have_is_refused`, `a_nutrient_whose_unit_differs_is_refused` |
| Every silver row names a dataset in `bronze.usda_zip`, a record in `bronze.record`, or, for a Wikidata statement, an item in `bronze.wikidata_revision`; and each kept nutrient row and portion names the bronze row it came from. | `every_silver_row_traces_to_the_bytes_it_came_from`: DuckDB refuses foreign keys across schemas, so a test holds this link |
| `docs/coverage.md`, `docs/diet.md`, `docs/ingredients.md`, `docs/crops.md`, `docs/portions.md` and `docs/data-catalogue.md` are what the warehouse writes, and every table and view says what it is. | `the_coverage_report_is_current`, `the_diet_report_is_current`, `the_ingredients_report_is_current`, `the_crops_report_is_current`, `the_portions_report_is_current`, `the_catalogue_is_current`, `every_table_and_view_says_what_it_is` |
| `diet/` holds four records and no other file: each USDA category's class, the groups of the organisms Foundation foods record, naming rules within a category, and exceptions for single foods. A rule or exception gives an answer and a reason; each category, organism, rule and exception is listed once; an organism is named by its NCBI id. | `the_diet_folder_holds_its_four_records_and_no_other_file`, `a_rule_or_exception_gives_an_answer_and_a_reason`, `diet_records_list_each_thing_once`, `a_taxon_is_named_by_its_ncbi_id` |
| Each food has a vegan and a vegetarian answer, yes, no or unknown, from the first that applies: an exception for it, a naming rule within its category, an animal organism (not vegan; and vegetarian unknown in a plant category), its category's class. Each answer names what decided it. | `every_food_has_an_answer_for_each_diet`, `the_diet_fixtures_hold`, and on rows written for the purpose, `each_food_s_answers_come_from_the_first_layer_that_applies` |
| The load stops on a category with no class, a class for a category no dataset has, an organism `diet/taxa.toml` lacks, a rule naming a category with no class or matching no food, a food two rules match, and an exception whose name is not USDA's or whose food silver lacks. | `the_load_refuses_what_the_diet_records_rule_out` |
| A plant category's food whose USDA name hides an animal ingredient is answered as its category says. | nothing: `unknown`. The answer's basis says it came from the category, and `docs/diet.md` says so. |
| An ingredient is a file `ingredients/<id>.toml`, its id lowercase letters and digits joined by hyphens; it has a name, at least one alias, and at least one form, one food per state; an alias is lowercase with single spaces and names one ingredient; a Wikidata item is `Q<digits>`. | `an_ingredient_is_named_by_its_file`, `an_alias_is_plain_and_names_one_ingredient`, `an_ingredient_has_one_food_per_state`, `a_wikidata_item_is_q_and_digits`, `the_ingredients_on_disk_load` |
| An alias is another name for its ingredient's foods, not for a near one: a name USDA gives a food of its own (Foundation's "Spinach, baby", "Onions, yellow, raw", "Oil, olive, extra virgin") belongs to an ingredient of its own. | nothing: review. `unknown` for any alias a review has not read. |
| Each form links a food in the pinned release by its fdc_id, with USDA's name for it word for word; the first form is the default. A form's FoodOn ids are derived from USDA's data, never typed. | `the_load_refuses_a_form_that_is_not_usda_s`, `every_ingredient_form_names_a_food_in_the_pinned_release`, and on rows written for the purpose, `foodon_ids_are_derived_and_candidates_are_names_not_yet_aliases` |
| An ingredient names its organism in an `[organism]` table, read from one place: a Foundation food, by its fdc_id and USDA's name for it; a Wikidata item `sources.toml` pins; or none, saying why. Each but none gives a taxon, written `NCBITaxon_<digits>` as `diet/taxa.toml` writes one, and a description goes with an fdc_id alone (decided with Jon, 2026-10-16: USDA first, Wikidata where USDA has none). | `an_ingredient_names_its_organism_or_why_it_has_none`, `an_organism_is_named_by_its_ncbi_id` |
| A Foundation food's organism is the "NCBI Taxon" USDA's data gives it, once however often USDA repeats it. A Wikidata item's is its own P685 (NCBI taxonomy ID), else the P685 of the one pinned item its P1582 (natural product of taxon) names, one step only; a deprecated statement is not read, and two read statements of a property, or one with no value, are refused. | `each_ingredient_s_organism_is_read_from_its_source`, `an_item_s_taxon_is_its_own_p685_else_its_product_of_s`, `an_item_whose_taxon_is_not_one_value_is_refused`, `statements_are_read_as_wikidata_writes_them` |
| The load stops on an organism its source does not give: a food named by other than USDA's name, one USDA's data records no NCBI Taxon for or another one for, a food not in the pinned release; a Wikidata item that is not pinned or gives another taxon; and on a pinned item no ingredient's or crop's organism reads, or pinned content that is another item's. | `the_load_refuses_an_organism_its_source_does_not_give`, and on the pinned data, `the_red_lentil_soup_s_organisms_are_read_from_their_sources` |
| Which Foundation food or Wikidata item an ingredient's organism is read from is a review's choice: red lentils read "Lentils, dry" and tofu a soy milk, as Foundation foods have neither of their own; parmesan reads its own Foundation food, whose NCBI Taxon is the cow's. Many ingredients can share one organism, and one organism can be several crops (USDA gives broccoli, kale and collards one taxon), so an organism does not say which ingredient a crop is. | nothing: review. `docs/ingredients.md` shows each ingredient's organism and where it was read from. That USDA gives the three crops one taxon: `one_organism_can_be_several_crops`. |
| An ingredient says in an `[origin]` table what it is made from: `harvested = true`, for a food as harvested, made from nothing; `made_from`, the ids of the ingredients it is made from, each once; or `none`, saying why, where what it is made from is no ingredient (decided 2026-10-17, Jon: "your call": the record says, and a review decides). `harvested` is never false. | `an_ingredient_says_what_it_is_made_from` |
| Each ingredient an ingredient is made from is in `ingredients/`, and has its organism where it has one; none leads back to it, directly or through others; a harvested ingredient has an organism. | `what_an_ingredient_is_made_from_is_there_shares_its_organism_and_leads_elsewhere`, and in silver, `each_ingredient_s_origin_is_loaded_and_its_keys_hold` |
| Whether an ingredient is harvested, and what it is made from, is a review's choice: red lentils are made from lentils, being lentils with the seed coat taken off; olive oil, pressed from olives as picked, and brown rice, hulled from paddy rice, name none, as USDA has no food for either as harvested. | nothing: review. `docs/ingredients.md` shows each ingredient's origin. |
| A crop is a file `crops/<id>.toml`, its id as an ingredient's is; it has a name, the Wikidata item of its organism (`Q<digits>`) with the NCBI Taxonomy id (`NCBITaxon_<digits>`) and the scientific name that item gives, and the ingredient it yields, by its id, with the state it is harvested in, raw or dry. | `a_good_crop_loads`, `a_crop_is_named_by_its_file`, `a_crop_names_its_organism_and_its_ingredient_by_their_ids`, `a_crop_is_harvested_raw_or_dry`, `the_crops_on_disk_load` |
| A crop's organism is its item's own P685, and its scientific name the item's own P225, each one statement. It is its ingredient's organism, or one step below it: the item's one P171 (parent taxon) names a pinned item whose own P685 is the ingredient's organism (decided 2026-10-16, Jon: "your call": Wikidata's parent taxon, not NCBI's dump, as the export is CC0 whole). | `a_crop_s_item_gives_its_own_taxon_and_its_parent_s`, `each_crop_s_organism_is_its_ingredient_s_or_one_step_below` |
| The load stops on a crop whose item is not pinned, gives another taxon or name, or lacks either; whose ingredient is not in `ingredients/`, has no organism, is not harvested, or has no form in the crop's state; and whose organism is neither its ingredient's nor one step below it: a parent taxon with another organism, two steps down, no parent taxon, or one not pinned. | `the_load_refuses_a_crop_its_organism_does_not_fit`, and on the pinned data, `the_crops_read_their_organisms_from_their_items` |
| Which ingredient a crop yields, and in which state, is a review's choice; several crops can yield one ingredient (tomato and cherry tomato yield tomato). | nothing: review. `docs/crops.md` shows each crop, what it yields and how its organism stands to its ingredient's. |
| An alias candidate is a name USDA's data gives a linked food (an SR Legacy common name, or a name in the parentheses of USDA's name, less notes such as "Includes foods for ...") that no ingredient lists; it stays unknown until a person reviews it into an ingredient's aliases. | `alias_candidates_come_from_usda_s_names`, `foodon_ids_are_derived_and_candidates_are_names_not_yet_aliases` |
| `units/` holds `units/count.toml` and no other file: count units, each with a key no other has and the names it may be written with, lowercase with single spaces, each naming one unit. A count unit has no grams of its own. The load stops on a count unit's name that cooklang's bundled units know. | `the_units_folder_holds_count_toml_alone`, `a_count_unit_record_refuses_what_it_does_not_know`, `a_count_unit_is_found_by_its_names`, `a_count_unit_may_not_be_a_unit_cooklang_knows`, `the_load_refuses_a_count_unit_cooklang_knows` |
| Every portion of a silver food is kept or rejected with its reason (an amount or gram weight not above 0), and each kept portion's unit is read from its text or listed in `docs/portions.md` by its leading phrase, never guessed. The text is its measure unit's name, or its modifier where its measure unit is "undetermined", as all of SR Legacy's are; its unit is the longest run of its leading phrase's first words that is a mass or volume unit cooklang's bundled units know, or a count unit. A unit later in the text is not the portion's. | `every_portion_is_read_or_listed`, `what_a_portion_s_unit_is_read_as`, `what_a_portion_s_unit_is_not_read_as`, `the_longest_run_that_names_a_unit_wins`, `fluid_ounce_portions_are_read_as_volume`, and on rows written for the purpose, `each_portion_s_unit_is_read_or_listed` |
| A portion in a mass unit whose grams differ from what the unit defines by more than 2% is kept as USDA gives it and listed in `docs/portions.md`. | on rows written for the purpose, `each_portion_s_unit_is_read_or_listed` |
| `export/datapackage.json` is what the warehouse writes: a Frictionless Data Package, version 2, of the gold views `EXPORTED` lists in `crates/warehouse/src/export.rs`, each a table resource with every column of its view, the view's and its columns' comments as descriptions, its rows inline as arrays, its columns' names first, then a row per line in primary-key order; and `_lineage`, the sha256 of each zip, Wikidata revision and record file the warehouse loaded. `crates/core` reads every value back equal to gold's. | `the_export_is_current`, `the_export_reads_back_as_gold`, and on views and text written for the purpose, `a_view_is_written_as_its_columns_names_then_a_row_per_line_in_key_order`, `the_rows_take_their_placeholder_s_place_a_step_past_its_line`, `a_column_of_a_type_the_export_does_not_write_is_refused` and `what_json_cannot_hold_or_gold_lacks_is_refused`. That the file conforms to the version 2 profile was seen on 2026-10-12 with the profile's JSON Schema and with frictionless-py 5.20.0, which checked its rows, keys and foreign keys; that check is not in the repo. |
| The export is licensed CC0-1.0 whole, and is not written while a publisher in `sources.toml` licenses its data otherwise. | `the_export_is_written_only_while_every_publisher_s_license_is_cc0` |
| `crates/core` reads the export into typed tables, and refuses one that is not a version 2 Data Package with the keys it knows, each once, a resource that is not a table or is listed twice, a field whose name another field of its resource has, or rows that break what the descriptor says: a first row that is not the fields' names, a value that is neither of its field's type nor null, a primary key repeated or null, a foreign key whose values are not all present and found in the table it names, or one that refers to fields other than that table's primary key. | `an_export_is_a_v2_data_package_with_keys_it_knows`, `each_row_has_a_value_of_its_field_s_type_in_each_field`, `a_primary_key_is_unique_and_never_null`, `a_foreign_key_finds_its_row`, `the_export_loads` |
| A recipe line's food is the ingredient one of whose aliases is its name, lowercased with single spaces, in the form whose state its note names in whole words ("as sold" for `as_sold`), else in its default form. A name that is no alias, or a note naming a state the ingredient has no form in, or naming two, gives the line no food. The states a note can name are the records' own. | `a_line_s_food_is_its_alias_s_ingredient_in_the_note_s_state`, `a_note_names_the_states_the_records_have` |
| A line's grams are the range of what the data gives, never one picked from it (decided with Jon, 2026-10-12): for a mass unit, by its definition in cooklang's bundled units; for a volume or count unit, read as a portion's unit is, by the food's USDA portions in it, or, where they disagree and the note's words are in some of their texts and not all, by those; for a volume unit the food has no portion in, by grams per millilitre from its portions in other volume units. A range amount gives a range. A line with no amount, an amount in text, no unit, a unit that is no mass, volume or count unit, or no portion to use has no grams, and says why. | `each_way_a_line_s_grams_are_reached`, `a_line_without_grams_says_why` |
| A nutrient's total is the sum, over the lines with grams whose food has a value for it, of the grams times the food's amount per 100 g, and says how many lines those are. | `a_total_says_how_many_lines_it_covers` |
| A recipe's vegan and vegetarian answers are the weakest of its lines': no if one is, else unknown if one is or a line has no food, else yes; a recipe with no line is unknown. Each answer names the lines that decided it. | `a_recipe_s_diet_is_its_weakest_line_s` |
| `recipes/` holds only `.cook` files, each parsing with no cooklang error or warning, with a title and a whole number of servings, 1 or more. Each has its report, `docs/recipes/<name>.md`, which is what `crates/core` writes from it and the export, and no report lacks its recipe. A report shows amounts to 0.1, a range by both its ends unless they show the same, and lists the lines without grams. | `a_recipe_record_parses_without_warnings`, `the_recipe_reports_are_current`, `amounts_are_shown_to_a_tenth_and_a_range_by_both_ends`, `a_report_lists_the_lines_without_grams` |
| Parsing records needs no database and no network. | `parsing_records_needs_no_database_or_network` |
| Reading the export and a recipe needs no database: `crates/core` depends on cooklang, serde and serde_json alone. | `reading_the_export_needs_no_database` |
| `balaena`, the command line, takes its data only from the export, which is built into it when it is compiled. `balaena analyze <recipe.cook>` prints the report `crates/core` writes for the recipe, saying it was printed by `balaena analyze`; `balaena search <words>...` prints the ingredients whose aliases, and the USDA foods whose names, have a word starting with each word given, the foods with the fewest words first, then by fdc_id, and each food's diet answers and the forms that link it; `balaena info` prints the license, the sources, the pinned zips and Wikidata revisions, and each table's rows of the export it carries. It exits with 2 on arguments it does not take (an unknown command, no word to search, an `-n` below 1), and with 1 on a recipe it cannot read or cooklang refuses, or a search with no word of letters or digits. | `analyze_prints_the_report_crates_core_writes`, `search_finds_aliases_and_foods_simplest_first`, `info_names_the_zips_and_revisions_the_export_came_from`, `the_cli_refuses_what_it_does_not_know` |
| The command line ships no database: `crates/balaena` depends on `balaena-core` and clap alone. | `the_cli_needs_no_database` |
| `main` changes only through a pull request that passes the Quality Gate on a branch up to date with `main`, with signed commits and linear history, and is never force-pushed or deleted. | the `main-protection` ruleset on GitHub, which lists no one who may bypass it, as its settings read on 2026-10-11. Nothing here tests the ruleset. |

The tests that span crates live in `crates/repo`, a member of the workspace in `Cargo.toml`. The
records' schemas are in `crates/records`, and the warehouse is `crates/warehouse`, with its layers
in `crates/warehouse/schema.sql`. Its tests read USDA's zips and the pinned Wikidata revisions from
the cache: fill it once with `cargo run -p balaena-warehouse -- fetch`. The Quality Gate fetches
with the warehouse's binary from the build its tests use, before them. Readers share `crates/core`,
which reads `export/datapackage.json` and analyses a recipe against it; the command line,
`crates/balaena`, builds the export into its binary.

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
3. **Vegan and vegetarian, by the societies' standards.** Vegan is The Vegan Society's definition:
   "dispensing with all products derived wholly or partly from animals", a diet "avoiding all animal
   foods such as meat (including fish, shellfish and insects), dairy, eggs and honey"
   (https://www.vegansociety.com/go-vegan/definition-veganism). Vegetarian is the Vegetarian
   Society's criteria (https://vegsoc.org/trademarks/trademark-criteria), which exclude "Animal
   rennet or any other by-product of slaughter", gelatine and isinglass, and "Animal carcass fats
   (including suet, lard or dripping)", and accept "Vegetarian rennet used in cheese-making". Both
   read on 2026-10-11. So honey is vegetarian and not vegan, and a cheese whose rennet USDA's name
   does not give is vegetarian: unknown. The cost: fewer answers are yes than under "no meat or
   fish".

### Decided with Jon, 2026-10-12

1. **A recipe line the data gives more than one weight shows the range.** When a food's USDA
   portions in a line's unit disagree and its note does not choose among them (a cup of raw onion
   is 160 g chopped and 115 g sliced), or its amount is a range (2-3 cloves), the report gives the
   least and the greatest and carries both into the totals; it never picks one. The cost: totals
   can be ranges.

### Decided with Jon, 2026-10-16

1. **An ingredient's organism is read from USDA first, then Wikidata.** Its NCBI Taxonomy id comes
   from the "NCBI Taxon" USDA's data gives a Foundation food; where no Foundation food has the
   organism, from a Wikidata item pinned to one revision. Wikidata's structured data is CC0, as our
   records are (its licensing page, read 2026-10-16). The GBIF Backbone Taxonomy is not a source:
   it is CC BY 4.0, and whether its bare ids may sit in CC0 records is the question still open for
   FoodOn's. The cost: Wikidata is a second upstream, pinned item by item, and an ingredient whose
   organism neither has stays without one.
2. **A crop's lineage is read from Wikidata's parent taxon, not NCBI's dump** (Jon: "your call",
   after NCBI's dump had first been recommended and agreed). NCBI's own lineage would be the direct
   referent for NCBI's ids, but its terms are not CC0: NCBI "places no restrictions on the use or
   distribution of the data", and warns that submitters "may claim patent, copyright, or other
   intellectual property rights" (www.ncbi.nlm.nih.gov/home/about/policies, read 2026-10-16). The
   export is not written while a publisher licenses its data otherwise than CC0, so pinning the dump
   would have meant weakening that rule. The cost: Wikidata's classification stands in for NCBI's,
   and a crop below its ingredient's species pins its parent's item too.

### Decided 2026-10-17

1. **What an ingredient is made from is its record's to say, and a review's to decide** (Jon: "your
   call"). Wikidata's P186 (made from material) was read for the seven ingredients made from
   something on 2026-10-17: lemon juice's names lemon, a food VBCI has since this step, and tofu's
   soy bean, beside soy milk and the soybean plant; olive oil's names the olive plant, vegetable
   broth's classes (winter vegetable, condiment, bean, cereal), parmesan's cow's milk and calf
   rennet, and brown rice and red lentils have none, only P279 (subclass of) rice and lentil. Read
   from Wikidata, two of the seven would have their link and five would be typed anyway. EFSA's
   FoodEx2 draws the same line: raw commodities are "the parts physically separated from a living
   source after harvesting (plants)", and derivatives "food products obtained from raw commodities
   by applying a process that changes the nature of food" (Eftimov, Korošec and Koroušić Seljak,
   *Nutrients* 9(6):542, 2017, read 2026-10-17). VBCI takes the line, not FoodEx2's data, which is
   reused with EFSA acknowledged. The cost: nothing but review holds whether an ingredient is
   harvested; the load holds what follows from it.

### Layers

| layer | where | what it is |
|---|---|---|
| records | `sources.toml`, `nutrients.toml`, `ingredients/`, `crops/`, `diet/`, `units/`, `recipes/<name>.cook` | The system of record, reviewed by pull request. |
| upstream | USDA's zips and Wikidata's revisions, fetched by `cargo run -p balaena-warehouse -- fetch` into the user's cache, never committed | Pinned in `sources.toml` by sha256 and length, a zip with its URL and release and a revision with its id, and each publisher's license, quoted terms, and the citation it asks for. |
| schemas and loaders | `crates/records` | Typed schemas that refuse a key they don't know. |
| warehouse | `crates/warehouse`, DuckDB, described in a generated `docs/data-catalogue.md` | Bronze holds every file as read, with its sha256; silver holds typed rows with keys; gold holds views that answer questions. It can be deleted at any time. |
| export | `export/datapackage.json`, generated from gold by `cargo run -p balaena-warehouse -- export` | What runtime readers load, stamped with the sha256s it came from. |
| readers | `crates/core` (analysis), `crates/balaena` (the CLI), later the app | They read the export, and `crates/core` parses a recipe with cooklang: a recipe a user writes is not a record, so it does not pass through the warehouse, and VBCI's own recipes are analysed as a user's is. DuckDB never ships in the CLI. |

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
5. **Diet status per food,** vegan and vegetarian, by the definitions decided above, each answer
   yes, no or unknown, from records reviewed by pull request: the first that applies of an exception
   for it (`diet/exceptions.toml`, 71 foods, each with its reason), a naming rule within its
   category (`diet/rules.toml`, 14 rules, such as names starting "Cheese" in Dairy and Egg Products:
   vegetarian unknown), the animal organism a Foundation food records (`diet/taxa.toml`, from USDA's
   NCBI taxon attribute), and its category's class (`diet/categories.toml`, all 28 categories). An
   exception sets only the diet its food's name gives evidence for: a name that names an animal
   ingredient gives "no", and a dish usually made with one, whose name does not say, gives
   "unknown". `docs/diet.md` counts the answers and what decided them. Reuses USDA's organism
   attributes; cook.md's diet-badges (MIT) and Open Food Facts' flags are references only. Done: the
   fixtures hold (oyster mushrooms, salsify and scallop squash vegan; duck and turkey eggs
   vegetarian, not vegan; fish oils not vegetarian; Parmesan vegetarian unknown),
   `the_diet_fixtures_hold`, `each_food_s_answers_come_from_the_first_layer_that_applies`,
   `the_load_refuses_what_the_diet_records_rule_out`, `the_diet_report_is_current`.
6. **Ingredient identity and aliases.** One file per ingredient, `ingredients/<id>.toml`: a name,
   the aliases a recipe may call it by, and forms, each a state (raw, dry, cooked, canned, frozen,
   or as sold) linked to one USDA food by fdc_id and USDA's name; the first form is the default. 16
   ingredients with 31 forms so far: the red lentil soup's eight and eight staples, all linked to SR
   Legacy foods; in the nine ingredients where a Foundation food was compared too, the SR Legacy one
   had values for more of our 16 nutrients, and `docs/ingredients.md` gives each form's count.
   FoodOn ids are derived from USDA's data, which gives them for Foundation foods only, so none of
   these SR Legacy forms has one yet. Alias candidates, unknown until reviewed, come from SR
   Legacy's common names (none of the 31 linked foods has one, measured) and from the parentheses in
   USDA's names ("bengal gram", for chickpeas); OpenCookbook's and Wikidata's need their sources
   pinned first, a later step. capycook's and open-tandoor-data's mappings come in only with their
   holders' permission. Done, with the Done approved on 2026-10-11 ("an alias two ingredients claim"
   in place of "an alias naming no ingredient", as aliases live in their ingredient's file): an
   alias two ingredients claim, a form linked to a food outside the pinned release, and a form whose
   name is not USDA's are refused (`an_alias_is_plain_and_names_one_ingredient`,
   `the_load_refuses_a_form_that_is_not_usda_s`), and `docs/ingredients.md` is current.
7. **Units and portions.** cooklang's bundled units for mass and volume, their spellings and
   `fl oz`, and count units as a record, `units/count.toml`, clove, bunch and head among them, none
   a name cooklang knows. cooklang 0.19.0 builds only with its `aisle` feature on (its
   `ingredient_list.rs` imports `aisle` whatever the features), so that is on too. USDA's portion
   text is read by cooklang's bundled converter, the one step 9 reads recipes with: SR Legacy writes
   every portion's unit in its modifier (its measure unit is "undetermined" throughout), Foundation
   in its measure unit. `gold.portion` gives grams per unit, and for a volume unit grams per
   millilitre; a food may have several portions in one unit (raw onion: "cup, chopped" 160 g, "cup,
   sliced" 115 g), and which one a recipe's line uses is step 9's to settle. Found on loading: 18 SR
   Legacy portions have amount 0, and are rejected; 25 portions in ounces or pounds differ from the
   unit's grams by more than 2% (USDA lists one ounce of instant oatmeal prepared with water at
   177 g), and are kept and listed. Done, with the tests in the rules above: every portion of a
   silver food is kept or rejected, every kept portion's unit is read or listed in
   `docs/portions.md`, which counts them, and the report is current.
8. **The export.** Gold written to one file that `crates/core` reads, with the sha256s it came from.
   The format reused is a Frictionless Data Package, version 2 (https://datapackage.org, read
   2026-10-12): `export/datapackage.json` holds eight gold views, foods with their diet answers,
   nutrients, amounts per 100 g, portions, count-unit names, ingredients, aliases and forms, each a
   table resource with its rows inline, one per line, so a diff of the file reads row by row. The
   sha256s are in `_lineage`, as the spec marks a property software writes with `_`; its own `hash`
   is a resource's hash of its own data, and these are hashes of the files the rows came from.
   Passed over: SQLite (binary diffs, and a C library in every reader), Parquet (binary, and the
   parquet crate in every reader), and CSV files with a descriptor (several files where this step
   says one). `crates/core` reads it with serde_json, which `Cargo.lock` already held. On 2026-10-12
   the file was about 9.0 MB, and 0.87 MB under gzip -9. Done, with the tests in the rules above: a
   test holds it to what the warehouse writes, and `crates/core` reads every value back equal to
   gold's.
9. **Analysis and recipe reports.** `crates/core` analyses a recipe, a record's or a user's,
   against the export (`src/recipe.rs`), with cooklang 0.19.0's parser and bundled units: each
   ingredient line finds its ingredient by alias and its form by the state its note names; its
   grams come from its unit's definition, from the food's USDA portions in its unit, or from grams
   per millilitre in the food's other volume portions; and its nutrients and diet answers come
   from the food. How sure each number is comes from how it was reached, never typed: each line
   says how, or why it has no grams; an amount the data does not pin to one number is a range
   (decided above); and each total says how many lines it covers. A line with no unit
   (`@onion{1}`) has no grams. `cargo run -p balaena-warehouse -- recipes` writes
   `docs/recipes/<name>.md` for each `recipes/<name>.cook`; the first is the red lentil soup of
   the 2026-10-11 draft, with its onion as `{1%medium}`. Done, with the tests in the rules above:
   the soup's report is current. On 2026-10-12 it answered vegan no (parmesan) and vegetarian
   unknown (parmesan, by its naming rule; vegetable broth, by its category), found the lentils dry
   and weighed the garlic by its clove portions.
10. **The CLI.** `crates/balaena`, the binary `balaena`: `balaena analyze <recipe.cook>`,
    `balaena search <words>... [-n N]` and `balaena info`, taking its data only from the export,
    which `include_str!` builds into the binary, so it carries the data it was built with and runs
    outside the repo. Its arguments are parsed by clap 4.6.7's builder with its default features
    off, which adds anstyle, clap, clap_builder and clap_lex to `Cargo.lock`. Searched on 2026-10-12
    and passed over: lexopt 0.3.2 (one crate, but help and errors by hand, and MIT alone), bpaf
    0.9.28 (one crate, with 8.5 million downloads to clap's 1.2 billion), pico-args 0.5.0 and
    gumdrop 0.8.1 (no release since 2022), argh 0.1.19 (BSD-3-Clause, and 11 crates), and clap with
    its derive (about 20 crates). On 2026-10-12, a macOS arm64 release build was 11.2 MB; each
    command took 0.11 to 0.12 s, but the first run after the build 0.46 s. `search` is for writing
    aliases and forms: no number is derived from what it finds. There is no `--json` or `--servings`
    yet: JSON output would be a format to hold, added when the app needs one. Done, with the tests
    in the rules above.

The second arc, farm reference data, began on 2026-10-16.

11. **Each ingredient's organism.** Each ingredient names its organism by NCBI Taxonomy id, read
    from a Foundation food or a pinned Wikidata item, or says why it has none (decided above). Every
    form of the 16 ingredients links an SR Legacy food, and SR Legacy records no taxon; of the 469
    Foundation foods, 359 record an "NCBI Taxon", 142 taxa in all, none for one food twice (scratch
    check, 2026-10-16; `silver.usda_food_organism` holds the 359, and its key the last). 13
    ingredients read a Foundation food, red lentils "Lentils, dry" and tofu a soy milk among them;
    lemon juice and cumin read Wikidata's lemon juice and cumin seed items, each through its natural
    product of taxon; vegetable broth has none. Wikidata's pages give no plain content for an item,
    so a revision's content comes from its revisions API, and its sha256 is pinned. Reuses
    serde_json and ureq. Done, with the tests in the rules above.
12. **Crops.** Each crop names the ingredient it yields and the state it is harvested in, and its
    organism through a pinned Wikidata item, which must be its ingredient's organism or one step
    below it by the item's parent taxon (decided above). Ten crops yield nine ingredients; carrot,
    as the cultivated subspecies, and cherry tomato are one step below their ingredient's species.
    Lemon, olive, soybean and rice waited for ingredients they yield as harvested; lemon juice,
    olive oil, tofu and brown rice are made from them (step 13 adds lemon and soybeans). Measured on
    2026-10-16: NCBI's lineage puts cultivated carrot, cherry tomato, cauliflower and Brussels
    sprouts one step below their species, and Wikidata's parent taxon agreed for all four (scratch
    check). The OpenFarm rescue's 340 CC0 records (`docs/prior-art.md`) are not loaded: their
    growing fields have no reader yet. Done, with the tests in the rules above.
13. **What each ingredient is made from.** Each ingredient says it is harvested, made from other
    ingredients, or made from what is no ingredient and why (decided above), and a crop yields only
    a harvested ingredient, raw or dry: until this step, nothing but review held that a crop yields
    its ingredient as harvested. Of the 18 ingredients, 11 are harvested; lemon juice, tofu and red
    lentils are made from lemon, soybeans and lentils; olive oil, brown rice, parmesan and vegetable
    broth name none. Lemon ("Lemons, raw, without peel") and soybeans ("Soybeans, mature seeds,
    raw") are new, so the lemon and soybean crops load. Olive and rice still wait: USDA's two pinned
    releases have no food for olives as picked, their olives being canned, pickled or stuffed, or
    for paddy rice (searched by description, 2026-10-17). The lemon's organism is read from
    Wikidata's lemon fruit item, as Foundation foods have no lemon, and soybeans' from the soy milk
    tofu's is. Done, with the tests in the rules above.

**Later:** the rest of farm reference data. A grower's weight per bunch or head comes from harvests
that record both a count and a weight, so it is measured, not typed; the 2026-10-11 search found
none stored in farmOS, Open Food Network, LiteFarm, FarmBot or Growstuff. Harvests are operational
data, so they wait for its design. Interchange through farmOS's JSON:API and the Data Food
Consortium's, which Open Food Network uses. Growstuff's crops carry GBIF and Wikidata ids and nest
under a parent crop, and AGROVOC links GBIF. A crop more than one step below its ingredient's
organism, such as a cultivar under a variety, is refused until a rule for it is decided. Olive
and rice crops wait for a USDA food for each as harvested.

### Grades

Tentzhen's grades, where they apply: a USDA value is `trusted`, an entry per pinned dataset in a
trusted base; a farm's own lab result is `measured`; a fuzzy match or a portion borrowed from
another food is `unknown` until someone reviews it; a derived number takes the weakest grade of
what it rests on. Step 9 shows how each number was reached, not a grade: it uses no fuzzy match
and borrows no portion from another food, so of the grades above only `trusted` could apply.

### Open

- Whether bare FoodOn ids may sit in CC0 records: `unknown`.
- The license of the Open Food Facts ingredients taxonomy: `unknown`.
- USDA asks to be told about a product that uses FoodData Central's data (its terms, quoted in
  `sources.toml`): a step on Jon's account, once VBCI ships one.
- A crate published for others to depend on would pass its `=` pins on to them. How to pin one is
  decided if a crate is ever published.
