-- The VBCI warehouse, loaded in order by crates/warehouse (src/lib.rs):
--
--   bronze  every file exactly as read: the records, every CSV in USDA's pinned zips, as text, and
--           each pinned Wikidata revision's content
--   silver  typed rows with keys, foreign keys and checks; each names the dataset or record it came
--           from. A nutrient or portion row of a silver food that silver does not keep is in
--           silver.usda_rejected with its reason; any other value that does not cast stops the load
--   gold    views that answer questions; never stored, always derived
--
-- The warehouse is rebuilt from the records, the pinned zips and the pinned Wikidata revisions on
-- every load, so it can be deleted at any time. Every table and view says what it is in a COMMENT
-- ON, which docs/data-catalogue.md is generated from
-- (`cargo run -p balaena-warehouse -- catalogue`).
--
-- Each CSV in a zip becomes a table bronze.usda_<name>, created by the loader: one row for each of
-- the CSV's rows, every column text, and a `dataset` column naming the zip it came from.

CREATE SCHEMA bronze;

CREATE TABLE bronze.record (
    path TEXT PRIMARY KEY,
    body TEXT NOT NULL
);
COMMENT ON TABLE bronze.record IS 'Every record file the loader read, exactly as read.';
COMMENT ON COLUMN bronze.record.path IS 'The file''s path, relative to the repo root.';

CREATE VIEW bronze.record_hash AS
SELECT path, sha256(body) AS sha256 FROM bronze.record;
COMMENT ON VIEW bronze.record_hash IS 'Lineage: the sha256 of each record file''s text.';

CREATE TABLE bronze.usda_zip (
    dataset TEXT PRIMARY KEY,
    url     TEXT NOT NULL,
    sha256  TEXT NOT NULL CHECK (length(sha256) = 64),
    bytes   BIGINT NOT NULL CHECK (bytes > 0)
);
COMMENT ON TABLE bronze.usda_zip IS 'Each dataset''s zip as sources.toml pins it. The loader refuses a zip whose length or sha256 differs.';
COMMENT ON COLUMN bronze.usda_zip.dataset IS 'The dataset''s id in sources.toml.';

CREATE TABLE bronze.usda_member (
    dataset   TEXT NOT NULL REFERENCES bronze.usda_zip (dataset),
    member    TEXT NOT NULL,
    bytes     BIGINT NOT NULL CHECK (bytes >= 0),
    sha256    TEXT NOT NULL CHECK (length(sha256) = 64),
    loaded_as TEXT,
    PRIMARY KEY (dataset, member)
);
COMMENT ON TABLE bronze.usda_member IS 'Every file in each dataset''s zip, with its length and sha256.';
COMMENT ON COLUMN bronze.usda_member.member IS 'The file''s path inside the zip.';
COMMENT ON COLUMN bronze.usda_member.loaded_as IS 'The bronze table a CSV was loaded into; NULL for a file that is not CSV, such as a workbook describing the fields.';

CREATE TABLE bronze.wikidata_revision (
    item     TEXT PRIMARY KEY CHECK (regexp_full_match(item, 'Q[0-9]+')),
    revision BIGINT NOT NULL CHECK (revision > 0),
    sha256   TEXT NOT NULL CHECK (length(sha256) = 64),
    bytes    BIGINT NOT NULL CHECK (bytes > 0),
    content  TEXT NOT NULL
);
COMMENT ON TABLE bronze.wikidata_revision IS 'Each Wikidata item sources.toml pins, with the content Wikidata stores for its pinned revision: the item''s JSON, exactly as read. The loader refuses content whose length or sha256 differs.';

-- ---------------------------------------------------------------- silver

CREATE SCHEMA silver;

-- A silver row names the dataset (bronze.usda_zip) or the record (bronze.record) it came from.
-- DuckDB refuses foreign keys across schemas, so that link is held by the warehouse's test
-- every_silver_row_traces_to_the_bytes_it_came_from, not by a key.

CREATE TABLE silver.usda_category (
    dataset     TEXT NOT NULL,
    id          INTEGER NOT NULL,
    description TEXT NOT NULL,
    PRIMARY KEY (dataset, id)
);
COMMENT ON TABLE silver.usda_category IS 'Each food category of each dataset, from food_category.csv.';

CREATE TABLE silver.usda_food (
    fdc_id      INTEGER PRIMARY KEY,
    dataset     TEXT NOT NULL,
    data_type   TEXT NOT NULL CHECK (data_type IN ('sr_legacy_food', 'foundation_food')),
    description TEXT NOT NULL,
    category_id INTEGER NOT NULL,
    FOREIGN KEY (dataset, category_id) REFERENCES silver.usda_category (dataset, id)
);
COMMENT ON TABLE silver.usda_food IS 'Every SR Legacy and Foundation food, meat included, from food.csv. The Foundation zip''s sample and acquisition rows stay in bronze.';
COMMENT ON COLUMN silver.usda_food.fdc_id IS 'USDA''s FoodData Central id, unique across both datasets.';

CREATE TABLE silver.usda_nutrient (
    dataset TEXT NOT NULL,
    id      INTEGER NOT NULL,
    name    TEXT NOT NULL,
    unit    TEXT NOT NULL,
    nbr     TEXT,
    PRIMARY KEY (dataset, id),
    UNIQUE (dataset, nbr)
);
COMMENT ON TABLE silver.usda_nutrient IS 'Each nutrient of each dataset, from nutrient.csv. Keyed by dataset: the same id can name a nutrient differently in two releases.';
COMMENT ON COLUMN silver.usda_nutrient.nbr IS 'USDA''s nutrient_nbr, as text exactly as written (269.3 is not 269); NULL where USDA gives none.';

CREATE TABLE silver.usda_food_nutrient (
    fdc_id      INTEGER NOT NULL REFERENCES silver.usda_food (fdc_id),
    dataset     TEXT NOT NULL,
    nutrient_id INTEGER NOT NULL,
    amount      DOUBLE NOT NULL,
    source_id   TEXT NOT NULL,
    PRIMARY KEY (fdc_id, nutrient_id),
    UNIQUE (dataset, source_id),
    FOREIGN KEY (dataset, nutrient_id) REFERENCES silver.usda_nutrient (dataset, id)
);
COMMENT ON TABLE silver.usda_food_nutrient IS 'Each amount of a nutrient in 100 g of a silver food, from food_nutrient.csv.';
COMMENT ON COLUMN silver.usda_food_nutrient.source_id IS 'The row''s id in food_nutrient.csv, as written.';

CREATE TABLE silver.usda_rejected (
    source  TEXT NOT NULL,
    dataset TEXT NOT NULL,
    id      TEXT NOT NULL,
    reason  TEXT NOT NULL CHECK (trim(reason) <> ''),
    PRIMARY KEY (source, dataset, id)
);
COMMENT ON TABLE silver.usda_rejected IS 'Each nutrient or portion row of a silver food that silver did not keep, and why: for a nutrient row, no amount, a nutrient its dataset lacks, a repeat of an earlier row, or a disagreement with another; for a portion, an amount or gram weight not above 0.';
COMMENT ON COLUMN silver.usda_rejected.source IS 'The bronze table the row is in.';
COMMENT ON COLUMN silver.usda_rejected.id IS 'The row''s id in that table, as written.';

CREATE TABLE silver.nutrient (
    nutrient TEXT PRIMARY KEY,
    position INTEGER NOT NULL UNIQUE CHECK (position >= 1),
    name     TEXT NOT NULL,
    unit     TEXT NOT NULL,
    record   TEXT NOT NULL
);
COMMENT ON TABLE silver.nutrient IS 'Each nutrient VBCI reports, from nutrients.toml, in the record''s order.';

CREATE TABLE silver.nutrient_code (
    nutrient TEXT NOT NULL REFERENCES silver.nutrient (nutrient),
    priority INTEGER NOT NULL CHECK (priority >= 1),
    code     TEXT NOT NULL UNIQUE,
    PRIMARY KEY (nutrient, priority)
);
COMMENT ON TABLE silver.nutrient_code IS 'Each USDA nutrient_nbr a nutrient is found by, first choice first.';

CREATE TABLE silver.usda_food_taxon (
    fdc_id  INTEGER NOT NULL REFERENCES silver.usda_food (fdc_id),
    dataset TEXT NOT NULL,
    taxon   TEXT NOT NULL CHECK (regexp_full_match(taxon, 'NCBITaxon_[0-9]+')),
    PRIMARY KEY (fdc_id, taxon)
);
COMMENT ON TABLE silver.usda_food_taxon IS 'The source organism''s group each Foundation food records, from food_attribute.csv''s "NCBI Taxon Parent", once per food; a food recorded as NA has none.';

CREATE TABLE silver.usda_food_organism (
    fdc_id  INTEGER PRIMARY KEY REFERENCES silver.usda_food (fdc_id),
    dataset TEXT NOT NULL,
    taxon   TEXT NOT NULL CHECK (regexp_full_match(taxon, 'NCBITaxon_[0-9]+'))
);
COMMENT ON TABLE silver.usda_food_organism IS 'The source organism each Foundation food records, from food_attribute.csv''s "NCBI Taxon", once per food however often USDA repeats it; a food recorded as NA has none, and the key refuses a food with two.';

CREATE TABLE silver.wikidata_statement (
    item     TEXT NOT NULL,
    property TEXT NOT NULL CHECK (property IN ('P685', 'P1582')),
    position INTEGER NOT NULL CHECK (position >= 1),
    value    TEXT,
    rank     TEXT NOT NULL CHECK (rank IN ('preferred', 'normal', 'deprecated')),
    PRIMARY KEY (item, property, position)
);
COMMENT ON TABLE silver.wikidata_statement IS 'Each statement of P685 (NCBI taxonomy ID) and P1582 (natural product of taxon) in each pinned Wikidata item''s content, in its order there.';
COMMENT ON COLUMN silver.wikidata_statement.value IS 'For P685 the NCBI Taxonomy id''s digits, for P1582 the item it names; NULL where the statement says some value or no value rather than giving one.';

CREATE TABLE silver.diet_category (
    description TEXT PRIMARY KEY,
    class       TEXT NOT NULL CHECK (class IN ('plant', 'dairy_and_egg', 'flesh', 'mixed')),
    record      TEXT NOT NULL
);
COMMENT ON TABLE silver.diet_category IS 'The class of each USDA category, from diet/categories.toml: plant, dairy_and_egg, flesh or mixed.';

CREATE TABLE silver.diet_taxon (
    taxon    TEXT PRIMARY KEY,
    name     TEXT NOT NULL,
    organism TEXT NOT NULL CHECK (organism IN ('animal', 'plant', 'fungus')),
    record   TEXT NOT NULL
);
COMMENT ON TABLE silver.diet_taxon IS 'The group of each organism a Foundation food records, from diet/taxa.toml.';

CREATE TABLE silver.diet_rule (
    category    TEXT NOT NULL REFERENCES silver.diet_category (description),
    starts_with TEXT NOT NULL CHECK (starts_with <> ''),
    vegan       TEXT CHECK (vegan IN ('yes', 'no', 'unknown')),
    vegetarian  TEXT CHECK (vegetarian IN ('yes', 'no', 'unknown')),
    why         TEXT NOT NULL CHECK (trim(why) <> ''),
    record      TEXT NOT NULL,
    PRIMARY KEY (category, starts_with),
    CHECK (vegan IS NOT NULL OR vegetarian IS NOT NULL)
);
COMMENT ON TABLE silver.diet_rule IS 'Each naming rule, from diet/rules.toml: within its category, a food whose USDA name starts with starts_with takes the answers the rule gives.';

CREATE TABLE silver.diet_exception (
    fdc_id      INTEGER PRIMARY KEY REFERENCES silver.usda_food (fdc_id),
    description TEXT NOT NULL,
    vegan       TEXT CHECK (vegan IN ('yes', 'no', 'unknown')),
    vegetarian  TEXT CHECK (vegetarian IN ('yes', 'no', 'unknown')),
    why         TEXT NOT NULL CHECK (trim(why) <> ''),
    record      TEXT NOT NULL,
    CHECK (vegan IS NOT NULL OR vegetarian IS NOT NULL)
);
COMMENT ON TABLE silver.diet_exception IS 'Each exception for one food, from diet/exceptions.toml, with the reason.';

CREATE TABLE silver.usda_food_foodon (
    fdc_id  INTEGER NOT NULL REFERENCES silver.usda_food (fdc_id),
    dataset TEXT NOT NULL,
    foodon  TEXT NOT NULL CHECK (regexp_full_match(foodon, 'FOODON_[0-9]+')),
    PRIMARY KEY (fdc_id, foodon)
);
COMMENT ON TABLE silver.usda_food_foodon IS 'Each FoodOn id a food carries, from food_attribute.csv''s "FoodOn Ontology ID For FDC Item"; Foundation foods carry them, SR Legacy''s bulk files do not.';

CREATE TABLE silver.usda_food_common_name (
    fdc_id      INTEGER NOT NULL REFERENCES silver.usda_food (fdc_id),
    dataset     TEXT NOT NULL,
    common_name TEXT NOT NULL CHECK (trim(common_name) <> ''),
    PRIMARY KEY (fdc_id, common_name)
);
COMMENT ON TABLE silver.usda_food_common_name IS 'Each "Common Name" attribute (type 1000) a food carries, as written: often several names, comma-separated, and sometimes a tag rather than a name.';

CREATE TABLE silver.ingredient (
    ingredient TEXT PRIMARY KEY CHECK (regexp_full_match(ingredient, '[a-z0-9]+(-[a-z0-9]+)*')),
    name       TEXT NOT NULL CHECK (trim(name) <> ''),
    wikidata   TEXT CHECK (wikidata IS NULL OR regexp_full_match(wikidata, 'Q[0-9]+')),
    record     TEXT NOT NULL
);
COMMENT ON TABLE silver.ingredient IS 'Each of VBCI''s own ingredients, from ingredients/<id>.toml.';

CREATE TABLE silver.ingredient_alias (
    alias      TEXT PRIMARY KEY CHECK (alias <> '' AND alias = lower(alias) AND alias = trim(alias)),
    ingredient TEXT NOT NULL REFERENCES silver.ingredient (ingredient)
);
COMMENT ON TABLE silver.ingredient_alias IS 'Each name a recipe may call an ingredient by; an alias names one ingredient.';

CREATE TABLE silver.ingredient_form (
    ingredient  TEXT NOT NULL REFERENCES silver.ingredient (ingredient),
    state       TEXT NOT NULL CHECK (state IN ('raw', 'dry', 'cooked', 'canned', 'frozen', 'as_sold')),
    position    INTEGER NOT NULL CHECK (position >= 1),
    fdc_id      INTEGER NOT NULL REFERENCES silver.usda_food (fdc_id),
    description TEXT NOT NULL,
    PRIMARY KEY (ingredient, state),
    UNIQUE (ingredient, position)
);
COMMENT ON TABLE silver.ingredient_form IS 'Each form of an ingredient: a state linked to one USDA food in the pinned release, with USDA''s name for it. Position 1 is the default.';

CREATE TABLE silver.ingredient_organism (
    ingredient  TEXT PRIMARY KEY REFERENCES silver.ingredient (ingredient),
    taxon       TEXT CHECK (taxon IS NULL OR regexp_full_match(taxon, 'NCBITaxon_[0-9]+')),
    fdc_id      INTEGER REFERENCES silver.usda_food (fdc_id),
    description TEXT,
    wikidata    TEXT CHECK (wikidata IS NULL OR regexp_full_match(wikidata, 'Q[0-9]+')),
    taxon_item  TEXT CHECK (taxon_item IS NULL OR regexp_full_match(taxon_item, 'Q[0-9]+')),
    none        TEXT,
    CHECK ((fdc_id IS NOT NULL)::INTEGER + (wikidata IS NOT NULL)::INTEGER + (none IS NOT NULL)::INTEGER = 1),
    CHECK ((taxon IS NULL) = (none IS NOT NULL)),
    CHECK ((description IS NULL) = (fdc_id IS NULL)),
    CHECK ((taxon_item IS NULL) = (wikidata IS NULL))
);
COMMENT ON TABLE silver.ingredient_organism IS 'Each ingredient''s organism, from its file''s [organism] table: an NCBI Taxonomy id read from a Foundation food or a pinned Wikidata item, or none and why. The load refuses a taxon its source does not give.';
COMMENT ON COLUMN silver.ingredient_organism.taxon_item IS 'Derived, never typed: the Wikidata item whose P685 gave the taxon, the one named or the one its P1582 names.';

CREATE TABLE silver.usda_measure_unit (
    dataset TEXT NOT NULL,
    id      INTEGER NOT NULL,
    name    TEXT NOT NULL,
    PRIMARY KEY (dataset, id)
);
COMMENT ON TABLE silver.usda_measure_unit IS 'Each measure unit of each dataset, from measure_unit.csv. SR Legacy''s portions all use 9999, "undetermined", and write their unit in modifier.';

CREATE TABLE silver.usda_portion (
    dataset         TEXT NOT NULL,
    id              TEXT NOT NULL,
    fdc_id          INTEGER NOT NULL REFERENCES silver.usda_food (fdc_id),
    amount          DOUBLE NOT NULL CHECK (amount > 0),
    measure_unit_id INTEGER NOT NULL,
    modifier        TEXT,
    gram_weight     DOUBLE NOT NULL CHECK (gram_weight > 0),
    unit_text       TEXT NOT NULL,
    PRIMARY KEY (dataset, id),
    FOREIGN KEY (dataset, measure_unit_id) REFERENCES silver.usda_measure_unit (dataset, id)
);
COMMENT ON TABLE silver.usda_portion IS 'Each portion of a silver food, from food_portion.csv: amount units of it weigh gram_weight grams.';
COMMENT ON COLUMN silver.usda_portion.unit_text IS 'The text its unit is read from: the measure unit''s name, or modifier where the unit is undetermined.';

CREATE TABLE silver.count_unit (
    unit   TEXT PRIMARY KEY,
    record TEXT NOT NULL
);
COMMENT ON TABLE silver.count_unit IS 'Each count unit, from units/count.toml: a unit a recipe counts in, which no mass or volume unit converts.';

CREATE TABLE silver.count_unit_name (
    name TEXT PRIMARY KEY,
    unit TEXT NOT NULL REFERENCES silver.count_unit (unit)
);
COMMENT ON TABLE silver.count_unit_name IS 'Each name a count unit may be written with.';

CREATE TABLE silver.usda_portion_unit (
    dataset  TEXT NOT NULL,
    id       TEXT NOT NULL,
    phrase   TEXT NOT NULL,
    kind     TEXT CHECK (kind IN ('mass', 'volume', 'count')),
    unit     TEXT,
    per_unit DOUBLE CHECK (per_unit IS NULL OR per_unit > 0),
    PRIMARY KEY (dataset, id),
    FOREIGN KEY (dataset, id) REFERENCES silver.usda_portion (dataset, id),
    CHECK ((kind IS NULL) = (unit IS NULL)),
    CHECK ((kind IN ('mass', 'volume')) = (per_unit IS NOT NULL))
);
COMMENT ON TABLE silver.usda_portion_unit IS 'The unit each portion''s text is read as, by crates/warehouse (src/portions.rs) with cooklang''s bundled units and units/count.toml: one row for every portion, kind NULL where no unit is read.';
COMMENT ON COLUMN silver.usda_portion_unit.phrase IS 'For a read unit, the words that named it; otherwise the text''s leading phrase, up to its first comma or parenthesis.';
COMMENT ON COLUMN silver.usda_portion_unit.per_unit IS 'Grams in one mass unit, or millilitres in one volume unit, by cooklang''s units.';

-- ---------------------------------------------------------------- gold

CREATE SCHEMA gold;

CREATE VIEW gold.food_nutrient AS
SELECT DISTINCT ON (f.fdc_id, c.nutrient)
    f.fdc_id, f.dataset, c.nutrient, fn.amount, n.unit, c.code
FROM silver.usda_food f
JOIN silver.usda_nutrient n ON n.dataset = f.dataset
JOIN silver.nutrient_code c ON c.code = n.nbr
JOIN silver.usda_food_nutrient fn ON fn.fdc_id = f.fdc_id AND fn.nutrient_id = n.id
ORDER BY f.fdc_id, c.nutrient, c.priority;
COMMENT ON VIEW gold.food_nutrient IS 'Each food''s amount of each nutrient VBCI reports, per 100 g: from the first of the nutrient''s codes the food has a value for.';
COMMENT ON COLUMN gold.food_nutrient.code IS 'The USDA nutrient_nbr the amount came from.';

CREATE VIEW gold.nutrient_coverage AS
SELECT f.dataset, n.nutrient, n.position, n.unit,
       count(*) AS foods, count(v.amount) AS with_value
FROM silver.usda_food f
CROSS JOIN silver.nutrient n
LEFT JOIN gold.food_nutrient v ON v.fdc_id = f.fdc_id AND v.nutrient = n.nutrient
GROUP BY ALL;
COMMENT ON VIEW gold.nutrient_coverage IS 'For each dataset and nutrient VBCI reports: how many of its silver foods have a value.';

CREATE VIEW gold.nutrient_source AS
SELECT dataset, nutrient, code, count(*) AS foods
FROM gold.food_nutrient
GROUP BY ALL;
COMMENT ON VIEW gold.nutrient_source IS 'For each dataset and nutrient: how many foods took their value from each code.';

CREATE VIEW gold.food_diet AS
WITH food AS (
    SELECT f.fdc_id, f.dataset, f.description, c.description AS category, d.class
    FROM silver.usda_food f
    JOIN silver.usda_category c ON c.dataset = f.dataset AND c.id = f.category_id
    JOIN silver.diet_category d ON d.description = c.description
), organism AS (
    SELECT ft.fdc_id, bool_or(t.organism = 'animal') AS animal
    FROM silver.usda_food_taxon ft
    JOIN silver.diet_taxon t ON t.taxon = ft.taxon
    GROUP BY ft.fdc_id
), layers AS (
    SELECT f.*, coalesce(o.animal, false) AS animal,
           r.starts_with, r.vegan AS rule_vegan, r.vegetarian AS rule_vegetarian,
           x.vegan AS exception_vegan, x.vegetarian AS exception_vegetarian
    FROM food f
    LEFT JOIN organism o ON o.fdc_id = f.fdc_id
    LEFT JOIN silver.diet_rule r ON r.category = f.category AND starts_with(f.description, r.starts_with)
    LEFT JOIN silver.diet_exception x ON x.fdc_id = f.fdc_id
)
SELECT fdc_id, dataset, description, category,
    CASE WHEN exception_vegan IS NOT NULL THEN exception_vegan
         WHEN rule_vegan IS NOT NULL THEN rule_vegan
         WHEN animal THEN 'no'
         WHEN class = 'plant' THEN 'yes'
         WHEN class = 'mixed' THEN 'unknown'
         ELSE 'no' END AS vegan,
    CASE WHEN exception_vegan IS NOT NULL THEN 'exception'
         WHEN rule_vegan IS NOT NULL THEN 'rule: ' || starts_with
         WHEN animal THEN 'organism: animal'
         ELSE 'category: ' || class END AS vegan_basis,
    CASE WHEN exception_vegetarian IS NOT NULL THEN exception_vegetarian
         WHEN rule_vegetarian IS NOT NULL THEN rule_vegetarian
         WHEN animal AND class = 'plant' THEN 'unknown'
         WHEN class = 'flesh' THEN 'no'
         WHEN class = 'mixed' THEN 'unknown'
         ELSE 'yes' END AS vegetarian,
    CASE WHEN exception_vegetarian IS NOT NULL THEN 'exception'
         WHEN rule_vegetarian IS NOT NULL THEN 'rule: ' || starts_with
         WHEN animal AND class = 'plant' THEN 'organism: animal'
         ELSE 'category: ' || class END AS vegetarian_basis
FROM layers;
COMMENT ON VIEW gold.food_diet IS 'Each food''s vegan and vegetarian answers, yes, no or unknown, and what decided each: an exception, a naming rule, an animal organism, or the category''s class, the first that applies.';
COMMENT ON COLUMN gold.food_diet.vegan IS 'By The Vegan Society''s definition: no product derived wholly or partly from animals.';
COMMENT ON COLUMN gold.food_diet.vegetarian IS 'By the Vegetarian Society''s criteria: no meat or fish, and no slaughter by-product such as animal rennet, gelatine or carcass fat.';

CREATE VIEW gold.ingredient_form AS
SELECT f.ingredient, f.state, f.position = 1 AS is_default, f.fdc_id, u.dataset, u.description,
       (SELECT string_agg(o.foodon, ', ' ORDER BY o.foodon)
        FROM silver.usda_food_foodon o WHERE o.fdc_id = f.fdc_id) AS foodon,
       (SELECT count(*) FROM gold.food_nutrient n WHERE n.fdc_id = f.fdc_id) AS nutrients
FROM silver.ingredient_form f
JOIN silver.usda_food u ON u.fdc_id = f.fdc_id;
COMMENT ON VIEW gold.ingredient_form IS 'Each form of each ingredient, with its USDA food, whether it is the default, the FoodOn ids that food carries, and how many of the nutrients VBCI reports it has a value for.';
COMMENT ON COLUMN gold.ingredient_form.foodon IS 'Derived, never typed: the FoodOn ids USDA''s data gives the linked food.';

CREATE VIEW gold.alias_candidate AS
WITH named AS (
    SELECT f.ingredient, f.fdc_id, 'SR Legacy common name' AS source,
           trim(lower(unnest(string_split(c.common_name, ',')))) AS candidate
    FROM silver.ingredient_form f
    JOIN silver.usda_food_common_name c ON c.fdc_id = f.fdc_id
    UNION ALL
    SELECT f.ingredient, f.fdc_id, 'USDA''s name, in parentheses',
           trim(lower(unnest(string_split(p.part, ','))))
    FROM silver.ingredient_form f
    JOIN silver.usda_food u ON u.fdc_id = f.fdc_id,
         unnest(regexp_extract_all(u.description, '\(([^)]*)\)', 1)) AS p(part)
)
SELECT DISTINCT ingredient, candidate, source, fdc_id
FROM named
WHERE candidate <> '' AND NOT starts_with(candidate, 'includes')
  AND candidate NOT IN (SELECT alias FROM silver.ingredient_alias);
COMMENT ON VIEW gold.alias_candidate IS 'Names USDA''s data gives a linked food that no ingredient lists as an alias yet: unknown until a person reviews one into an ingredient''s aliases. From SR Legacy''s common names and from the parentheses in USDA''s names, less notes such as "Includes foods for ...".';

CREATE VIEW gold.portion AS
SELECT p.dataset, p.id, p.fdc_id, p.amount, u.unit, u.kind, p.gram_weight,
       p.gram_weight / p.amount AS grams_per_unit,
       CASE WHEN u.kind = 'volume' THEN p.gram_weight / (p.amount * u.per_unit) END AS grams_per_ml,
       CASE WHEN u.kind = 'mass' THEN p.gram_weight / (p.amount * u.per_unit) END AS mass_ratio,
       p.unit_text, p.modifier
FROM silver.usda_portion p
JOIN silver.usda_portion_unit u ON u.dataset = p.dataset AND u.id = p.id
WHERE u.kind IS NOT NULL;
COMMENT ON VIEW gold.portion IS 'Each portion whose unit is read: grams in one of its units, and for a volume unit the food''s grams per millilitre. A food may have several portions in one unit, told apart by their text and modifier ("cup, chopped" and "cup, sliced").';
COMMENT ON COLUMN gold.portion.mass_ratio IS 'For a mass unit, the portion''s grams over what the unit defines: 1 where USDA''s gram weight agrees with the unit.';

CREATE VIEW gold.portion_unread AS
SELECT p.dataset, u.phrase, count(*) AS portions, min(p.unit_text) AS example
FROM silver.usda_portion p
JOIN silver.usda_portion_unit u ON u.dataset = p.dataset AND u.id = p.id
WHERE u.kind IS NULL
GROUP BY ALL;
COMMENT ON VIEW gold.portion_unread IS 'The portions whose unit no mass, volume or count unit reads, by the leading phrase of their text.';

CREATE VIEW gold.portion_mass_disagreement AS
SELECT p.*, f.description
FROM gold.portion p
JOIN silver.usda_food f ON f.fdc_id = p.fdc_id
WHERE p.kind = 'mass' AND abs(p.mass_ratio - 1) > 0.02;
COMMENT ON VIEW gold.portion_mass_disagreement IS 'Portions in a mass unit whose gram weight differs from what the unit defines by more than 2%: a finding about USDA''s data, kept, not refused.';

-- The views below bring the records' tables the export carries into gold, so it reads gold alone.

CREATE VIEW gold.nutrient AS
SELECT nutrient, position, name, unit FROM silver.nutrient;
COMMENT ON VIEW gold.nutrient IS 'Each nutrient VBCI reports, from nutrients.toml, with its position in the record, its name and the unit its amounts are in.';

CREATE VIEW gold.ingredient AS
SELECT ingredient, name, wikidata FROM silver.ingredient;
COMMENT ON VIEW gold.ingredient IS 'Each of VBCI''s own ingredients, by id, with its name and Wikidata item.';

CREATE VIEW gold.ingredient_organism AS
SELECT ingredient, taxon, fdc_id, wikidata, taxon_item, none FROM silver.ingredient_organism;
COMMENT ON VIEW gold.ingredient_organism IS 'Each ingredient''s organism by its NCBI Taxonomy id, and where the id was read from: a Foundation food, or a Wikidata item that sources.toml pins; or why it has none. Many ingredients can share one organism.';
COMMENT ON COLUMN gold.ingredient_organism.taxon IS 'The NCBI Taxonomy id, as NCBITaxon_<digits>; NULL for an ingredient with none.';
COMMENT ON COLUMN gold.ingredient_organism.fdc_id IS 'The Foundation food whose "NCBI Taxon" in USDA''s data gave the taxon.';
COMMENT ON COLUMN gold.ingredient_organism.wikidata IS 'The Wikidata item the ingredient names, at the revision sources.toml pins.';
COMMENT ON COLUMN gold.ingredient_organism.taxon_item IS 'The Wikidata item whose P685 (NCBI taxonomy ID) gave the taxon: the one named, or the one its P1582 (natural product of taxon) names.';
COMMENT ON COLUMN gold.ingredient_organism.none IS 'Why the ingredient has no organism, as its file says.';

CREATE VIEW gold.ingredient_alias AS
SELECT alias, ingredient FROM silver.ingredient_alias;
COMMENT ON VIEW gold.ingredient_alias IS 'Each name a recipe may call an ingredient by, and the ingredient it names.';

CREATE VIEW gold.count_unit_name AS
SELECT name, unit FROM silver.count_unit_name;
COMMENT ON VIEW gold.count_unit_name IS 'Each name a count unit may be written with, and the count unit it names: the unit gold.portion gives a portion counted in it.';
