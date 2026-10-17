# Prior art

What already exists for each part of VBCI, who built it, and what we take from it. We build with
existing building blocks where they are not the point, and make friends with the people who made
them, as tentzhen does. A proposal for a new piece starts here, and searches again.

## How this was searched

- **When and how:** 2026-10-11. Three searches by research agents, one per question: open
  standards for identifying foods and nutrients; tools that already link recipes to USDA data; farm
  and farm-to-table software. They read repos through GitHub's API, pages with curl, and Wikidata
  through its SPARQL endpoint. The same day, scratch scripts compared four USDA downloads (SR
  Legacy 2018-04; Foundation 2025-04-24, 2025-12-18 and 2026-04-30) and cooklang 0.19.0's source.
- **How the sources were read:** a source marked **re-read** was checked again outside the agents on
  2026-10-11, for what the mark names: a license and last push through GitHub's API, a file through
  GitHub's code search, a count from a repo's data at a named commit or through Wikidata's SPARQL
  endpoint, or a sentence from the publisher's own page. The rest is as the agents reported it, so
  re-read a source before resting anything on it.
- **Not seen:** GitHub's repository search matches only names and descriptions, and its code search
  was rate-limited (one batch failed). Kaggle had one web search. USDA's API host did not resolve
  from the agents' sandbox, so FDC ids were sorted against the CSV downloads instead. Papers behind
  paywalls, and sources in languages other than English, were not searched.
- **What it can show:** what these searches found. It cannot show that nothing closer exists.

## The short answer

- **No open tool found links Cooklang recipes to USDA data.** Cooklang's own discussion of nutrition
  (spec #52, opened 2021-10-26) is open, and its maintainers say it waits on units and density.
  cook.md, a commercial Cooklang company, runs a hosted nutrition service.
- **Two tools share most of our design.** capycook (Go, MIT) has a hand-picked mapping of 243 names
  to FDC ids, gram weights from `food_portion`, and USDA's zips pinned by sha256, the Foundation one
  by the sha256 we measured. recipya-rs (Rust, AGPL-3.0) loads FDC into Postgres and takes grams
  from `food_portion`.
- **Curated mappings from ingredient names to foods exist,** but mostly under licenses a CC0 record
  cannot take in without permission: open-tandoor-data (ODbL), capycook (MIT), Open Food Facts'
  taxonomy (no license stated). OpenCookbook's 55 entries (49 with an fdc_id; CC0) and Wikidata
  (CC0) can come in as they are.
- **The searches found no crate on crates.io that reads FDC data, and no maintained loader that puts
  it in DuckDB,** so plan steps 3 and 4 are new work.
- **Identifiers:** USDA's own Foundation files carry FoodOn ids, from an open food ontology (CC BY
  4.0). INFOODS tagnames, the nutrient standard, are licensed for non-commercial use and the newest
  file dates from 2012. Wikidata is CC0 but thin.
- **Farm software:** none of farmOS, Open Food Network, LiteFarm, FarmBot or Growstuff stores a
  fixed weight per bunch or head. Growstuff records both a count and a weight for each harvest.

## Linking recipes to USDA data

| project | what it is | license, activity | what we take | source |
|---|---|---|---|---|
| cooklang-rs, CookCLI | Cooklang's parser and its command-line tool | MIT; pushed 2026-10-03 and 2026-10-11 | the parser and its bundled units (steps 7 and 9). Neither computes nutrition, as reported. Two open requests touch it: cookcli#299, "Support nutrition metadata and scale it with servings" (opened 2026-04-02), and cookcli#459, showing nutrition metadata on the recipe page (2026-08-25) | github.com/cooklang/cooklang-rs and github.com/cooklang/cookcli, **re-read** (license, push, the two issues) |
| capycook | Go: maps 243 ingredient names to FDC ids (about 190 picked by hand), with 714 gram weights from `food_portion`; vendors a subset of SR Legacy and Foundation 2026-04-30 | MIT, pushed 2026-07-31 | its design, which has the same three parts as ours (a hand-picked mapping, gram weights from `food_portion`, zips pinned by sha256); its mapping only with its author's permission, since our records are CC0. Its `data/usda/PROVENANCE.md` pins the Foundation zip by the sha256 we measured (`70457ee9…`) | github.com/ogngnaoh/capycook, **re-read** (license, push, sha256) |
| open-tandoor-data | Tandoor's food data: 391 foods, 390 with an fdc_id (132 in SR Legacy, 104 in Foundation 2026-04-30, 154 in neither), names in many languages, 163 unit conversions | ODbL schema, DbCL contents (its README), pushed 2026-10-03 | nothing without permission: ODbL is share-alike | github.com/TandoorRecipes/open-tandoor-data, **re-read** (license, push; the fdc_id counts at commit b4d05271bb) |
| OpenCookbook `nutrition-db.json` | 55 ingredients with aliases, 49 of them with an fdc_id | CC0-1.0, pushed 2026-04-18 | seed alias candidates (step 6) | github.com/JPEGtheDev/OpenCookbook, **re-read** (license, push, counts) |
| ingredient-parser | Python: parses ingredient lines, and matches them offline against bundled Foundation, SR Legacy and FNDDS foods by fusing BM25, embedding and fuzzy rankings | MIT, pushed 2026-10-11 | a reference for matching, if matching past reviewed aliases is ever needed | github.com/strangetom/ingredient-parser, **re-read** (license, push) |
| recipya-rs | Rust: loads FDC's JSON into Postgres; takes the top full-text hit; grams from a `food_portion` modifier, else "medium" | AGPL-3.0, pushed 2026-10-11 | ideas only: the license rules out copying | github.com/reaper47/recipya-rs, **re-read** (license, push) |
| gourd | Go: about 60 ingredient-to-FDC mappings with aliases and densities; 19 of its 59 ids are SR Legacy or Foundation | MIT, archived | nothing | github.com/nickysemenza/gourd, **re-read** (license, archived) |
| cook.md nutrition | a hosted service; its client crates `cooklang-reports-nutrition` and `cookmd-nutrition-client` leave the matching to the server | crates MIT, created 2026-07-18 | nothing: the matching is on their server | crates.io, as reported |
| Cooklang spec #52 | "Nutrition Information", opened 2021-10-26, last activity 2026-04-22 | — | a place to offer our CC0 mapping once it exists | github.com/cooklang/spec, **re-read** (title, last activity) |

Also reported: Tandoor itself (GitHub reads its license as NOASSERTION) has the user pick a food
from FDC's API search; the nutrition subset of Recipe1M+ is for research only and carries no ids;
no paper from 2023 to 2026 released a set of ingredients matched to FDC foods.

## Reading USDA's data

| project | what it is | license, activity | what we take | source |
|---|---|---|---|---|
| DuckDB `read_csv` | reads the six CSVs of each dataset with `all_varchar` and `strict_mode` | MIT | bronze (step 3) | scratch check with DuckDB 1.5.6: counts equal Python's `csv` module and, for SR Legacy, USDA's own record counts |
| usda_fooddata | a `schema.sql` typed by hand for all 34 tables of FDC's 2020-10 release | Apache-2.0, pushed 2021-03-04 | a check on silver's column types (step 4) | github.com/dbready/usda_fooddata, **re-read** (license, push) |
| other loaders | into Postgres (natej/foody, GPL-3.0), or for Branded foods only | — | nothing | as reported |
| crates.io | searched for fdc, usda, fooddata, "food data central", nutrient and nutrition | — | no crate that reads FDC data | as reported |

## Identifiers for foods and nutrients

| standard | what it is | license, upkeep | what we take | source |
|---|---|---|---|---|
| FoodOn | an OBO food ontology: about 29,000 classes, plus the organisms foods come from (NCBITaxon), and states such as raw, cooked, canned | CC BY 4.0; last push 2026-09-20 | its ids as cross-references (step 6), taken from USDA's files, which give one for 365 of Foundation 2026-04-30's 469 foods | OBO registry, **re-read** (license); FDC's `food_attribute.csv`, scratch check |
| Wikidata | P12917 "FoodData Central ID" (156 statements on 129 items) and P1978 "USDA NDB number" (1,470 on 596) | CC0 for structured data | cross-references and seed candidates (step 6); too thin and noisy for a key (reported: Q3406628 "onion" names a frozen boiled onion) | query.wikidata.org, **re-read** (counts, license sentence) |
| INFOODS tagnames | FAO's identifiers for food components (ENERC, PROCNT, FAT, CHOCDF, …) | FAO's site terms allow copying for "non-commercial products or services"; newest file saved 2012-11-06 | nothing: our nutrient keys are USDA's codes (step 4) | fao.org, **re-read** (terms sentence) |
| LanguaL, FoodEx2 | thesauri for describing foods (LanguaL 2017; EFSA's FoodEx2) | LanguaL: no reuse license; FoodEx2: reuse with EFSA acknowledged | nothing | as reported |
| Open Food Facts | the database, and its ingredients taxonomy: 691 entries with an FDC code, vegan and vegetarian flags on about 230 | database ODbL; the taxonomy file states no license | nothing until the taxonomy's license is known; Wikidata's P5930 links its ingredients | world.openfoodfacts.org/data, **re-read** (ODbL sentence) |

## Units and diet flags

| source | what it is | license | what we take | source of this row |
|---|---|---|---|---|
| cooklang's bundled units | `units.toml`: mass and volume with their spellings, `fl oz` among them; no count units | MIT | step 7 | cooklang 0.19.0's source, scratch check |
| USDA's organism attributes | Foundation's `food_attribute.csv` gives a food's source organism and its group, for 365 of the 2026-04-30 release's 469 foods: plants (NCBITaxon 35493), fungi (4751), animals (33208), or NA for mixed foods | CC0, by USDA's API guide (quoted in `sources.toml`) | step 5 | scratch check |
| diet-badges | a cook.md plugin: vegan, vegetarian and pescatarian badges from word lists, with look-alike exceptions such as "beef tomatoes" and "chicken of the woods" | MIT (its own `LICENSE`) | a reference for step 5's fixtures; its lists only with its notice | github.com/cook-md/plugins, **re-read** (license) |

## Farm and farm-to-table software

For the plan's "Later": none of it is built on yet.

| project | what it is | license, activity | crops, harvests, units | source |
|---|---|---|---|---|
| farmOS | farm records on Drupal, with a JSON:API | GPL-2.0, pushed 2026-10-09 | plant types are free-text terms per farm; a quantity has a measure, a value and a unit term with no gram weight | github.com/farmOS/farmOS, **re-read** (license, push) |
| Open Food Network | a farm-to-table marketplace; products through the Data Food Consortium's API | AGPL-3.0, pushed 2026-10-11 | an "items" variant, such as a bunch, has no gram weight; categories map to DFC product types | github.com/openfoodfoundation/openfoodnetwork, **re-read** (license, push) |
| Data Food Consortium taxonomies | SKOS product types and measures, "bunch" among them | AGPL-3.0 | interchange, later | github.com/datafoodconsortium/taxonomies, **re-read** (license) |
| LiteFarm | farm management, about 320 crop types | GPL-3.0, pushed 2026-10-08 | harvests in kg, mt, lb or t; where its crop data comes from is not stated | github.com/LiteFarmOrg/LiteFarm, **re-read** (license, push) |
| FarmBot | an open farming robot | MIT, pushed 2026-10-09 | 248 crop files, copied from OpenFarm | github.com/FarmBot/Farmbot-Web-App, **re-read** (license, push) |
| OpenFarm | a crop database whose servers shut down in April 2025; its README puts the data in the public domain (CC0); a community repo (CC0-1.0) rescued 340 records | MIT code, archived | possible CC0 crop data; the rescue's contents not inspected | github.com/openfarmcc/OpenFarm, **re-read** (archived); github.com/thefullnacho/openfarm-crops-rescue, **re-read** (license) |
| Growstuff | community gardening records, with a JSON:API | AGPL-3.0 code; API data CC BY-SA 3.0 | a harvest stores a count unit and a weight; crops carry GBIF and Wikidata ids | github.com/Growstuff/growstuff, **re-read** (license, push) |
| Mycodo | monitoring and control of growing conditions | GPL-3.0, pushed 2026-08-03 | no crop or harvest records seen | github.com/kizniche/Mycodo, **re-read** (license, push) |
| AGROVOC | FAO's thesaurus: over 41,400 concepts, with URIs and a SPARQL endpoint | CC BY 4.0 for six languages | plants and their products are separate concepts, labelled in the plural ("kales"); it links GBIF | fao.org/agrovoc, **re-read** (license sentence) |

A GitHub search for hydroponics, aquaponics, vertical farms and greenhouse management found
hardware controllers and student repos, read by their descriptions only; none of those says it
keeps crop, harvest or unit records.

## Organisms, and aeroponics

For plan step 11 and the farm reference data after it. Searched on 2026-10-16 without agents:
USDA's pinned Foundation zip read with a scratch script, Wikidata through its SPARQL endpoint and
API, each publisher's own page for its license, and GitHub's repository search. A source marked
**re-read** was read that day for what the mark names.

| source | what it is | license | what VBCI takes | source |
|---|---|---|---|---|
| USDA's "NCBI Taxon" | Foundation's `food_attribute.csv` names each food's own organism beside its group: 359 of the 2026-04-30 release's 469 foods, 142 taxa, and NA for 6 more | CC0, by USDA's API guide (quoted in `sources.toml`) | step 11's organisms, first | scratch check |
| Wikidata | items for foods and for taxa: a food's natural product of taxon (P1582) names a taxon's item, which carries its NCBI taxonomy ID (P685) and a GBIF taxon ID (P846) | CC0 for structured data | step 11's organisms where USDA has none, each item pinned to a revision | wikidata.org/wiki/Wikidata:Licensing, **re-read** (license sentence) |
| NCBI Taxonomy | the ids USDA and Wikidata use; monthly dumps, `new_taxdump_<date>.zip`, 155 MB on 2026-10-01 | NCBI "places no restrictions on the use or distribution of the data contained therein" | its ids; not its dump, which is not CC0 (decided 2026-10-16, "Crops" below) | ncbi.nlm.nih.gov/home/about/policies, **re-read** (sentence); ftp.ncbi.nlm.nih.gov/pub/taxonomy/taxdump_archive, **re-read** (listing) |
| GBIF Backbone Taxonomy | GBIF's taxonomy, DOI 10.15468/39omei, last published 2023-08-28 | CC BY 4.0 | nothing: not CC0 (decided with Jon, 2026-10-16) | api.gbif.org/v1/dataset/d7dddbf4-2cf0-4f39-9b2a-bb099caae36c, **re-read** (license, dates) |

Eleven of the twelve NCBI ids step 11's ingredients read from USDA, all but soy's, were looked up in
Wikidata: each names one item, and each of those items carries one GBIF id.

A GitHub search for "aeroponic" and for "aeroponics", the 15 repos with the most stars for each,
read by their descriptions only, found DIY builds and hardware controllers, a project predicting
yield, one measuring plant height by camera, and two with no description; no description says it
keeps crop, harvest or unit records. The 2026-10-11 search above did
not look for aeroponics: Jon's word then was "aquaponic", corrected to "aeroponic" on 2026-10-16.

## Crops

For plan step 12. Searched on 2026-10-16 without agents: the OpenFarm rescue's files at its last
commit, read with a scratch script; Growstuff's schema; Wikidata through its SPARQL endpoint; NCBI's
lineage through its E-utilities and its dated dump. A source marked **re-read** was read that day
for what the mark names.

| source | what it is | license | what VBCI takes | source |
|---|---|---|---|---|
| OpenFarm rescue | 340 crop records rebuilt from the Wayback Machine after OpenFarm.cc shut down in April 2025, each with the capture it came from; growing fields (sun, spacing, sowing); its author calls it "a useful scaffold, not an authority" | CC0-1.0 | nothing yet: its growing fields have no reader. Step 12's crops were chosen by VBCI's ingredients, and the rescue was checked for them | github.com/thefullnacho/openfarm-crops-rescue at 754cfd75, **re-read** (license, README, crops.json) |
| Growstuff's crop model | a crop nests under a parent crop (`parent_id`); its scientific names carry a GBIF key, rank and Wikidata id; it has alternate names by language, and a `public_food_key` into the Australian Food Classification | AGPL-3.0 code; API data CC BY-SA 3.0 | the shape: a crop names the food it yields | github.com/Growstuff/growstuff, `db/schema.rb` at 02f5f95b, **re-read** (tables) |
| NCBI Taxonomy dump | `taxdmp_2026-10-01.zip`, 79,650,311 bytes: `nodes.dmp`, 3.0 million lines, gives each id's parent and rank, and `names.dmp`, 5.2 million lines, its names | NCBI "places no restrictions on the use or distribution of the data", which is not CC0 | nothing: not pinned (`docs/architecture.md`, decided 2026-10-16) | ftp.ncbi.nlm.nih.gov/pub/taxonomy/taxdump_archive, **re-read** (listing, download) |

Of the rescue's 305 records with a scientific name, 237 are distinct once case, spacing, parentheses
and the hybrid sign are made one; by an exact match on Wikidata's taxon name, 178 name one taxon
item with an NCBI id, and 37 name none. Its carrot "var. sativus" names an item with no NCBI id;
NCBI's name is "Daucus carota subsp. sativus". Searched by name, it has crops for 11 of the 13
organisms of VBCI's plant ingredients, none for lentil or rice. Its other records include named
cultivars with no scientific name and a few duplicates (carrot and carrots).

## Made from

For plan step 13. Searched on 2026-10-17 without agents: Wikidata's items through its API, read as
they stood that day, not at pinned revisions; USDA's two pinned releases by description, with a
scratch script; FoodEx2 through one paper and one web search. A source marked **re-read** was read
that day for what the mark names.

| source | what it is | license | what VBCI takes | source |
|---|---|---|---|---|
| Wikidata's P186 (made from material) | a statement naming what an item is made from | CC0 | nothing: read from Wikidata, two of the seven ingredients made from something would have their link (`docs/architecture.md`, decided 2026-10-17) | the items below, **re-read** (P186, P279) |
| FoodEx2 | EFSA's food classification: raw commodities, "the parts physically separated from a living source after harvesting (plants)", and derivatives, "food products obtained from raw commodities by applying a process that changes the nature of food"; its facet F27, "source commodities", names what a food comes from | reuse with EFSA acknowledged | the line between harvested and made from, not its data | Eftimov, Korošec and Koroušić Seljak, *Nutrients* 9(6):542, 2017, pmc.ncbi.nlm.nih.gov/articles/5490521, **re-read** (both definitions); F27 as reported by a web search of EFSA's FoodEx2 slides |

What Wikidata's items for the seven say they are made from (P186), and what they are a subclass of
(P279):

| ingredient | item | P186 | P279 |
|---|---|---|---|
| lemon juice | Q1375049 | lemon (Q1093742, the fruit) | lemon and lime juice, juice |
| tofu | Q177378 | soy milk, soy bean (Q61503220, the food), soybean (Q11006, the plant) | soy cheese, meat alternative |
| olive oil | Q93165 | Olea europaea (Q37083, the plant) | vegetable oil, cooking oil, seed oil |
| vegetable broth | Q42366457, vegetable stock | winter vegetable, condiment, bean, cereal | broth, convenience food, soup |
| parmesan | Q155922 | cow's milk, calf rennet | Grana cheese, dairy product |
| brown rice | Q1066624 | none | rice |
| red lentils | Q67445480, red lentil | none | lentil |

Wikidata has no item for olives as picked that a search for "olive", "olives", "olive fruit" or
"table olive" found; "paddy rice" is Q110461730, which states neither P186 nor P279. USDA's pinned
releases have no food for either: their olives are canned, pickled or stuffed, and no food's
description names paddy, rough, husked or unmilled rice. That red lentils are lentils with the
seed coat taken off, which shows their red, and are mostly split too, is as a web search reported
it (recipe pages, and a 2005 Cereal Chemistry paper on lentil milling whose page answered 403), not
re-read.
