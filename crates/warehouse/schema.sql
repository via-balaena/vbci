-- The VBCI warehouse, loaded in order by crates/warehouse (src/lib.rs):
--
--   bronze  every file exactly as read: the records, and every CSV in USDA's pinned zips, as text
--
-- Silver and gold arrive with plan steps 4 onward (docs/architecture.md). The warehouse is rebuilt
-- from the records and the pinned zips on every load, so it can be deleted at any time.
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
