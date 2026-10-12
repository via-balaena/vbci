-- The VBCI warehouse, loaded in order by crates/warehouse (src/lib.rs):
--
--   bronze  every file exactly as read: the records, and every CSV in USDA's pinned zips, as text
--   silver  typed rows with keys, foreign keys and checks; each names the dataset or record it came
--           from. A nutrient row of a silver food that silver does not keep is in
--           silver.usda_rejected with its reason; any other value that does not cast stops the load
--   gold    views that answer questions; never stored, always derived
--
-- The warehouse is rebuilt from the records and the pinned zips on every load, so it can be deleted
-- at any time. Every table and view says what it is in a COMMENT ON, which docs/data-catalogue.md
-- is generated from (`cargo run -p balaena-warehouse -- catalogue`).
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
COMMENT ON TABLE silver.usda_rejected IS 'Each nutrient row of a silver food that silver did not keep, and why: no amount, a nutrient its dataset lacks, a repeat of an earlier row, or a disagreement with another.';
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
