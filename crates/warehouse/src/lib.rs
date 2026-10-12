//! VBCI's warehouse, in DuckDB: rebuilt from the records and USDA's pinned zips on every load, so it
//! can be deleted at any time. `schema.sql` describes its layers: bronze, silver and gold.

pub mod usda;

use balaena_records::{Nutrients, Sources};
use duckdb::{Connection, params};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs, process};

const SCHEMA: &str = include_str!("../schema.sql");

/// The repo's root, from this crate's place in it.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Where USDA's zips are cached: `$VBCI_CACHE`, else `$XDG_CACHE_HOME/vbci`, else
/// `$HOME/.cache/vbci`.
pub fn cache_dir() -> Result<PathBuf, String> {
    cache_dir_from(|key| env::var(key).ok().filter(|v| !v.is_empty()))
}

fn cache_dir_from(var: impl Fn(&str) -> Option<String>) -> Result<PathBuf, String> {
    if let Some(dir) = var("VBCI_CACHE") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = var("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(dir).join("vbci"));
    }
    var("HOME")
        .map(|home| PathBuf::from(home).join(".cache").join("vbci"))
        .ok_or_else(|| "no cache: set VBCI_CACHE, XDG_CACHE_HOME or HOME".to_string())
}

/// The bronze table a file in a zip is loaded into: `usda_<name>` for `<name>.csv`, or None for a
/// file that is not CSV. A CSV whose name is not lowercase letters, digits and underscores is
/// refused rather than renamed, so no two files can land in one table.
fn csv_table(member: &str) -> Result<Option<String>, String> {
    let file = member.rsplit('/').next().unwrap_or(member);
    let Some(stem) = file.strip_suffix(".csv") else {
        return Ok(None);
    };
    let plain = !stem.is_empty()
        && stem
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !plain {
        return Err(format!(
            "{member}: a CSV's name must be lowercase letters, digits and underscores"
        ));
    }
    Ok(Some(format!("usda_{stem}")))
}

/// `s` as a SQL string literal.
fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// A directory this process made under the system's temp dir, removed when dropped. Its name
/// carries a count kept by the process as well as the time: on macOS the clock advances by whole
/// microseconds, and two tests that made one in the same microsecond collided ("File exists",
/// 2026-10-11).
struct Scratch(PathBuf);

impl Scratch {
    /// The directory's name, from the process, the time and the process's count.
    fn name(process: u32, nanos: u128, n: u64) -> String {
        format!("balaena-warehouse-{process}-{nanos}-{n}")
    }

    fn new() -> Result<Self, String> {
        static MADE: AtomicU64 = AtomicU64::new(0);
        let n = MADE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = env::temp_dir().join(Self::name(process::id(), nanos, n));
        fs::create_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn sql(e: duckdb::Error) -> String {
    e.to_string()
}

/// Builds the warehouse in memory from the records under `root` and the pinned zips in `cache`.
/// Bronze takes `sources.toml` and `nutrients.toml` as read, each dataset's zip as pinned, every
/// file in each zip with its sha256, and every CSV row for row, as text; then silver and gold.
pub fn load(root: &Path, cache: &Path) -> Result<Connection, String> {
    let (sources, sources_text) = Sources::load(root)?;
    let (nutrients, nutrients_text) = Nutrients::load(root)?;
    let conn = Connection::open_in_memory().map_err(sql)?;
    conn.execute_batch(SCHEMA).map_err(sql)?;
    for (path, text) in [
        (Sources::PATH, &sources_text),
        (Nutrients::PATH, &nutrients_text),
    ] {
        conn.execute(
            "INSERT INTO bronze.record VALUES (?, ?)",
            params![path, text],
        )
        .map_err(sql)?;
    }
    let scratch = Scratch::new()?;
    let mut tables: BTreeMap<String, Vec<(String, PathBuf)>> = BTreeMap::new();
    for d in &sources.dataset {
        conn.execute(
            "INSERT INTO bronze.usda_zip VALUES (?, ?, ?, ?)",
            params![d.id, d.url, d.sha256, d.bytes as i64],
        )
        .map_err(sql)?;
        for m in usda::extract(cache, d, &scratch.0)? {
            let table = csv_table(&m.name)?;
            conn.execute(
                "INSERT INTO bronze.usda_member VALUES (?, ?, ?, ?, ?)",
                params![d.id, m.name, m.bytes as i64, m.sha256, table],
            )
            .map_err(sql)?;
            if let Some(table) = table {
                tables
                    .entry(table)
                    .or_default()
                    .push((d.id.clone(), m.file));
            }
        }
    }
    for (table, files) in &tables {
        let selects: Vec<String> = files
            .iter()
            .map(|(dataset, file)| {
                format!(
                    "SELECT {} AS dataset, * FROM read_csv({}, header = true, all_varchar = true, strict_mode = true)",
                    literal(dataset),
                    literal(&file.to_string_lossy())
                )
            })
            .collect();
        let stem = table.trim_start_matches("usda_");
        conn.execute_batch(&format!(
            "CREATE TABLE bronze.{table} AS {};\n\
             COMMENT ON TABLE bronze.{table} IS {};\n\
             COMMENT ON COLUMN bronze.{table}.dataset IS 'The dataset''s id in sources.toml: the zip the row came from.';",
            selects.join(" UNION ALL BY NAME "),
            literal(&format!(
                "Every row of {stem}.csv in each dataset's zip, as read: every column text."
            )),
        ))
        .map_err(|e| format!("bronze.{table}: {e}"))?;
    }
    load_silver(&conn, &nutrients)?;
    Ok(conn)
}

/// Types bronze's USDA rows into silver, keeping each nutrient row of a silver food or rejecting it
/// with its reason (`SILVER` says which), and loads `nutrients.toml`. Then refuses a nutrient code that some dataset lacks, or has
/// with another unit: a code is matched exactly, so a misspelt or rounded code would silently find
/// nothing, or another nutrient.
fn load_silver(conn: &Connection, nutrients: &Nutrients) -> Result<(), String> {
    conn.execute_batch(SILVER)
        .map_err(|e| format!("silver: {e}"))?;
    for (position, n) in nutrients.nutrient.iter().enumerate() {
        conn.execute(
            "INSERT INTO silver.nutrient VALUES (?, ?, ?, ?, ?)",
            params![n.key, position as i64 + 1, n.name, n.unit, Nutrients::PATH],
        )
        .map_err(|e| format!("silver.nutrient {}: {e}", n.key))?;
        for (priority, code) in n.codes.iter().enumerate() {
            conn.execute(
                "INSERT INTO silver.nutrient_code VALUES (?, ?, ?)",
                params![n.key, priority as i64 + 1, code],
            )
            .map_err(|e| format!("silver.nutrient_code {} {code}: {e}", n.key))?;
        }
    }
    let mut stmt = conn
        .prepare(
            "SELECT n.nutrient, c.code, z.dataset, n.unit, u.unit
             FROM silver.nutrient n
             JOIN silver.nutrient_code c USING (nutrient)
             CROSS JOIN bronze.usda_zip z
             LEFT JOIN silver.usda_nutrient u ON u.dataset = z.dataset AND u.nbr = c.code
             WHERE u.unit IS DISTINCT FROM n.unit
             ORDER BY n.position, c.priority, z.dataset",
        )
        .map_err(sql)?;
    let wrong: Vec<(String, String, String, String, Option<String>)> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .map_err(sql)?
        .collect::<Result<_, _>>()
        .map_err(sql)?;
    if let Some((nutrient, code, dataset, want, got)) = wrong.into_iter().next() {
        return Err(match got {
            None => format!(
                "{}: {nutrient}'s code {code} is not in {dataset}'s nutrient.csv",
                Nutrients::PATH
            ),
            Some(got) => format!(
                "{}: {nutrient}'s code {code} has unit {got} in {dataset}, not {want}",
                Nutrients::PATH
            ),
        });
    }
    Ok(())
}

/// Silver's USDA rows, typed from bronze. A food_nutrient row of a silver food is rejected, with
/// its reason, when it has no amount, names a nutrient its dataset's nutrient.csv lacks, repeats an
/// earlier row for the same food and nutrient with the same amount (the earliest row by id is
/// kept), or disagrees with another row for the same food and nutrient (every such row is
/// rejected, so the food has no value rather than a guess). Every other row is typed, and a value
/// that does not cast stops the load.
const SILVER: &str = "
INSERT INTO silver.usda_category
SELECT dataset, CAST(id AS INTEGER), description FROM bronze.usda_food_category;

INSERT INTO silver.usda_food
SELECT CAST(fdc_id AS INTEGER), dataset, data_type, description, CAST(food_category_id AS INTEGER)
FROM bronze.usda_food
WHERE data_type IN ('sr_legacy_food', 'foundation_food');

INSERT INTO silver.usda_nutrient
SELECT dataset, CAST(id AS INTEGER), name, unit_name, nullif(nutrient_nbr, '')
FROM bronze.usda_nutrient;

CREATE TEMP TABLE in_scope AS
SELECT b.dataset, b.id, CAST(b.fdc_id AS INTEGER) AS fdc_id,
       CAST(b.nutrient_id AS INTEGER) AS nutrient_id, b.amount,
       coalesce(b.amount, '') = '' AS no_amount,
       NOT EXISTS (SELECT 1 FROM silver.usda_nutrient n
                   WHERE n.dataset = b.dataset AND n.id = CAST(b.nutrient_id AS INTEGER)) AS no_nutrient
FROM bronze.usda_food_nutrient b
WHERE CAST(b.fdc_id AS INTEGER) IN (SELECT fdc_id FROM silver.usda_food);

CREATE TEMP TABLE typable AS
SELECT dataset, id, fdc_id, nutrient_id, CAST(amount AS DOUBLE) AS amount,
       min(CAST(amount AS DOUBLE)) OVER pair <> max(CAST(amount AS DOUBLE)) OVER pair AS conflicting,
       row_number() OVER (pair ORDER BY CAST(id AS BIGINT)) AS copy,
       first_value(id) OVER (pair ORDER BY CAST(id AS BIGINT)) AS first_id
FROM in_scope
WHERE NOT (no_amount OR no_nutrient)
WINDOW pair AS (PARTITION BY fdc_id, nutrient_id);

INSERT INTO silver.usda_rejected
SELECT 'usda_food_nutrient', dataset, id,
       concat_ws('; ', CASE WHEN no_amount THEN 'no amount' END,
                       CASE WHEN no_nutrient THEN 'nutrient ' || nutrient_id || ' is not in its dataset''s nutrient.csv' END)
FROM in_scope
WHERE no_amount OR no_nutrient
UNION ALL
SELECT 'usda_food_nutrient', dataset, id,
       'another row gives food ' || fdc_id || ' a different amount of nutrient ' || nutrient_id
FROM typable
WHERE conflicting
UNION ALL
SELECT 'usda_food_nutrient', dataset, id,
       'repeats row ' || first_id || ': same food, nutrient and amount'
FROM typable
WHERE NOT conflicting AND copy > 1;

INSERT INTO silver.usda_food_nutrient
SELECT fdc_id, dataset, nutrient_id, amount, id
FROM typable
WHERE NOT conflicting AND copy = 1;

DROP TABLE typable;
DROP TABLE in_scope;
";

/// Where `coverage` writes its report, relative to the repo root.
pub const COVERAGE_PATH: &str = "docs/coverage.md";

/// Where `catalogue` writes the data catalogue, relative to the repo root.
pub const CATALOGUE_PATH: &str = "docs/data-catalogue.md";

fn rows<T>(
    conn: &Connection,
    query: &str,
    f: impl FnMut(&duckdb::Row<'_>) -> duckdb::Result<T>,
) -> Result<Vec<T>, String> {
    let mut stmt = conn.prepare(query).map_err(|e| format!("{query}: {e}"))?;
    stmt.query_map([], f)
        .map_err(|e| format!("{query}: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{query}: {e}"))
}

/// `docs/coverage.md`, as the warehouse writes it: for each dataset, how many of its foods have a
/// value for each nutrient VBCI reports, and which codes the values came from; then the rows silver
/// rejected.
pub fn coverage_report(conn: &Connection, sources: &Sources) -> Result<String, String> {
    let mut out = String::from(
        "# Nutrient coverage\n\n\
         Generated by `cargo run -p balaena-warehouse -- coverage` from the datasets `sources.toml`\n\
         pins and the nutrients `nutrients.toml` lists; the test `the_coverage_report_is_current`\n\
         fails when this file differs from what that would write. Change the records, not this\n\
         file.\n\n\
         For each dataset: how many of its foods have a value for each nutrient, per 100 g\n\
         (`gold.nutrient_coverage`), and how many took it from each of the nutrient's USDA codes,\n\
         first choice first (`gold.nutrient_source`). Every food counts, meat included.\n",
    );
    for d in &sources.dataset {
        let coverage = rows(
            conn,
            &format!(
                "SELECT nutrient, unit, foods, with_value FROM gold.nutrient_coverage \
                 WHERE dataset = {} ORDER BY position",
                literal(&d.id)
            ),
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )?;
        let foods = coverage.first().map(|c| c.2).unwrap_or_default();
        out.push_str(&format!(
            "\n## {} {} (`{}`): {foods} foods\n\n\
             | nutrient | unit | foods with a value | share | from codes |\n|---|---|---|---|---|\n",
            d.name, d.release, d.id
        ));
        for (nutrient, unit, foods, with_value) in coverage {
            let sources = rows(
                conn,
                &format!(
                    "SELECT c.code, coalesce(s.foods, 0) FROM silver.nutrient_code c \
                     LEFT JOIN gold.nutrient_source s ON s.nutrient = c.nutrient \
                     AND s.code = c.code AND s.dataset = {} \
                     WHERE c.nutrient = {} ORDER BY c.priority",
                    literal(&d.id),
                    literal(&nutrient)
                ),
                |r| {
                    Ok(format!(
                        "{}: {}",
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?
                    ))
                },
            )?;
            let share = if foods == 0 {
                0.0
            } else {
                100.0 * with_value as f64 / foods as f64
            };
            let from = sources.join(", ");
            out.push_str(&format!(
                "| {nutrient} | {unit} | {with_value} | {share:.1}% | {from} |\n"
            ));
        }
    }
    let rejected = rows(
        conn,
        "SELECT dataset, source, reason, count(*) FROM silver.usda_rejected \
         GROUP BY ALL ORDER BY dataset, source, reason",
        |r| {
            Ok(format!(
                "| {} | {} | {} | {} |",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?
            ))
        },
    )?;
    out.push_str(
        "\n## Rows silver rejected\n\n\
         Nutrient rows of silver foods that silver did not keep (`silver.usda_rejected`).\n\n",
    );
    if rejected.is_empty() {
        out.push_str("None.\n");
    } else {
        out.push_str("| dataset | bronze table | reason | rows |\n|---|---|---|---|\n");
        for line in rejected {
            out.push_str(&line);
            out.push('\n');
        }
    }
    Ok(out)
}

/// `docs/data-catalogue.md`, as the warehouse writes it: every table and view, layer by layer, with
/// the comment it carries and its columns.
pub fn catalogue(conn: &Connection) -> Result<String, String> {
    let mut out = String::from(
        "# Data catalogue\n\n\
         Every table and view in the warehouse, as DuckDB describes them, with the comments\n\
         `crates/warehouse/schema.sql` and the loader give them. Generated by\n\
         `cargo run -p balaena-warehouse -- catalogue`; the test `the_catalogue_is_current` fails\n\
         when this file differs from what that would write. Change the schema, not this file.\n",
    );
    for schema in ["bronze", "silver", "gold"] {
        out.push_str(&format!("\n## {schema}\n"));
        let objects = rows(
            conn,
            &format!(
                "SELECT table_name, 'table', coalesce(comment, '') FROM duckdb_tables() \
                 WHERE schema_name = {s} AND NOT temporary \
                 UNION ALL \
                 SELECT view_name, 'view', coalesce(comment, '') FROM duckdb_views() \
                 WHERE schema_name = {s} AND NOT internal AND NOT temporary \
                 ORDER BY 1",
                s = literal(schema)
            ),
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )?;
        for (name, kind, comment) in objects {
            out.push_str(&format!("\n### {schema}.{name}\n\nA {kind}. {comment}\n\n"));
            let columns = rows(
                conn,
                &format!(
                    "SELECT column_name, data_type, coalesce(comment, '') FROM duckdb_columns() \
                     WHERE schema_name = {} AND table_name = {} ORDER BY column_index",
                    literal(schema),
                    literal(&name)
                ),
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )?;
            let (described, bare): (Vec<_>, Vec<_>) =
                columns.into_iter().partition(|(_, _, c)| !c.is_empty());
            if !described.is_empty() {
                out.push_str("| column | type | what it is |\n|---|---|---|\n");
                for (column, data_type, comment) in &described {
                    out.push_str(&format!("| {column} | {data_type} | {comment} |\n"));
                }
            }
            if !bare.is_empty() {
                let names: Vec<String> =
                    bare.iter().map(|(c, t, _)| format!("`{c}` {t}")).collect();
                let lead = if described.is_empty() {
                    "Columns"
                } else {
                    "\nOther columns"
                };
                out.push_str(&format!("{lead}: {}.\n", names.join(", ")));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use balaena_records::Dataset;
    use std::cell::Cell;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn dataset(bytes: &[u8]) -> Dataset {
        Dataset {
            id: "d".into(),
            publisher: "p".into(),
            name: "D".into(),
            release: "1".into(),
            url: "https://example.org/d.zip".into(),
            sha256: usda::sha256_hex(bytes),
            bytes: bytes.len() as u64,
        }
    }

    /// Two scratch dirs made at the same clock reading get different names. Making 256 in a row on
    /// one thread never collided even without the count, as each takes more than a microsecond;
    /// the collision came from tests on parallel threads, so the name is checked, not the timing.
    #[test]
    fn scratch_dirs_made_together_are_distinct() {
        assert_ne!(Scratch::name(7, 1_000, 0), Scratch::name(7, 1_000, 1));
        let dirs: Vec<Scratch> = (0..64).map(|_| Scratch::new().unwrap()).collect();
        let names: std::collections::BTreeSet<_> = dirs.iter().map(|d| d.0.clone()).collect();
        assert_eq!(names.len(), dirs.len());
    }

    #[test]
    fn what_counts_as_a_csv_table() {
        assert_eq!(csv_table("x/food.csv"), Ok(Some("usda_food".into())));
        assert_eq!(
            csv_table("food_nutrient.csv"),
            Ok(Some("usda_food_nutrient".into()))
        );
        assert_eq!(
            csv_table("x/Download & API Field Descriptions.xlsx"),
            Ok(None)
        );
        assert!(csv_table("x/Food Nutrient.csv").is_err());
        assert!(csv_table("x/.csv").is_err());
    }

    #[test]
    fn the_cache_follows_the_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(
            cache_dir_from(env(&[
                ("VBCI_CACHE", "/c"),
                ("XDG_CACHE_HOME", "/x"),
                ("HOME", "/h")
            ])),
            Ok(PathBuf::from("/c"))
        );
        assert_eq!(
            cache_dir_from(env(&[("XDG_CACHE_HOME", "/x"), ("HOME", "/h")])),
            Ok(PathBuf::from("/x/vbci"))
        );
        assert_eq!(
            cache_dir_from(env(&[("HOME", "/h")])),
            Ok(PathBuf::from("/h/.cache/vbci"))
        );
        assert!(cache_dir_from(env(&[])).is_err());
    }

    #[test]
    fn bytes_that_differ_from_the_pin_are_refused() {
        let good = b"the pinned zip";
        let d = dataset(good);
        assert_eq!(usda::verify(&d, good), Ok(()));
        let same_length = b"the pinned zap";
        assert!(
            usda::verify(&d, same_length)
                .unwrap_err()
                .contains("sha256")
        );
        assert!(usda::verify(&d, b"short").unwrap_err().contains("bytes"));

        let cache = Scratch::new().unwrap();
        let err = usda::fetch(std::slice::from_ref(&d), &cache.0, |_, _| {
            Ok(same_length.to_vec())
        })
        .unwrap_err();
        assert!(
            err.contains("sha256") && err.contains("downloaded from"),
            "{err}"
        );
        let left: Vec<_> = fs::read_dir(&cache.0).unwrap().collect();
        assert!(left.is_empty(), "a refused download left {left:?}");
    }

    #[test]
    fn a_cached_zip_that_differs_from_its_pin_is_refused() {
        let good = b"the pinned zip";
        let d = dataset(good);
        let cache = Scratch::new().unwrap();
        let path = usda::zip_path(&cache.0, &d);
        fs::write(&path, b"the pinned zap").unwrap();
        let called = Cell::new(false);
        let err = usda::fetch(std::slice::from_ref(&d), &cache.0, |_, _| {
            called.set(true);
            Ok(good.to_vec())
        })
        .unwrap_err();
        assert!(err.contains("cached at"), "{err}");
        assert!(
            !called.get(),
            "a cached zip that differs was replaced by a download"
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            b"the pinned zap",
            "the cached zip was changed"
        );
        let out = Scratch::new().unwrap();
        assert!(
            usda::extract(&cache.0, &d, &out.0)
                .unwrap_err()
                .contains("sha256")
        );
    }

    #[test]
    fn a_good_download_is_cached_under_its_sha256() {
        let good = b"the pinned zip";
        let d = dataset(good);
        let cache = Scratch::new().unwrap();
        let calls = Cell::new(0);
        let get = |url: &str, limit: u64| {
            calls.set(calls.get() + 1);
            assert_eq!((url, limit), (d.url.as_str(), d.bytes));
            Ok(good.to_vec())
        };
        let first = usda::fetch(std::slice::from_ref(&d), &cache.0, get).unwrap();
        assert_eq!(first, [("d".to_string(), usda::Fetched::Downloaded)]);
        assert_eq!(fs::read(usda::zip_path(&cache.0, &d)).unwrap(), good);
        let second = usda::fetch(std::slice::from_ref(&d), &cache.0, get).unwrap();
        assert_eq!(second, [("d".to_string(), usda::Fetched::Cached)]);
        assert_eq!(calls.get(), 1);
    }

    /// `load` checks each cached zip against its pin too: a file of the pinned length with other
    /// bytes, sitting where the zip should be, stops the load.
    #[test]
    fn loading_refuses_a_cached_zip_that_differs() {
        let (sources, _) = Sources::load(&repo_root()).unwrap();
        let cache = Scratch::new().unwrap();
        for d in &sources.dataset {
            fs::write(usda::zip_path(&cache.0, d), vec![0u8; d.bytes as usize]).unwrap();
        }
        let Err(err) = load(&repo_root(), &cache.0) else {
            panic!("a cached zip that differs from its pin was loaded");
        };
        assert!(err.contains("sha256") && err.contains("cached at"), "{err}");
    }

    /// A file in a zip whose path climbs out of its folder (`../`) is refused, and nothing is
    /// written outside the folder.
    #[test]
    fn a_member_that_would_extract_outside_its_folder_is_refused() {
        use std::io::Write;
        use zip::write::{SimpleFileOptions, ZipWriter};
        let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let stored =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer.start_file("../outside.csv", stored).unwrap();
        writer.write_all(b"a,b\n1,2\n").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let d = dataset(&bytes);
        let cache = Scratch::new().unwrap();
        fs::write(usda::zip_path(&cache.0, &d), &bytes).unwrap();
        let out = Scratch::new().unwrap();
        let dir = out.0.join("into");
        let err = usda::extract(&cache.0, &d, &dir).unwrap_err();
        assert!(err.contains("outside its folder"), "{err}");
        assert!(!dir.join("outside.csv").exists() && !out.0.join("outside.csv").exists());
    }

    /// Nutrients for the synthetic bronze below.
    const SYNTHETIC_NUTRIENTS: &str = r#"
[[nutrient]]
key = "energy"
name = "Energy"
unit = "KCAL"
codes = ["208", "958"]

[[nutrient]]
key = "sugars"
name = "Sugars"
unit = "G"
codes = ["269", "269.3"]
"#;

    /// A warehouse whose bronze is a few rows written here, with silver loaded from them by
    /// `nutrients`. Food 3 is a sample food, out of silver's scope. Rows: 1-2 give food 1 energy by
    /// 208 and 958; 3 gives food 2 energy by 958 only; 4 gives food 2 sugars by 269.3 only; 5 gives
    /// food 1 sugars by 269, and 6 repeats it; 7 and 8 give food 2 different sugars by 269; 9 names a
    /// nutrient nutrient.csv lacks; 10 has no amount; 11 is the sample food's.
    fn synthetic(nutrients: &str) -> Result<Connection, String> {
        let conn = Connection::open_in_memory().map_err(sql)?;
        conn.execute_batch(SCHEMA).map_err(sql)?;
        conn.execute_batch(&format!(
            "INSERT INTO bronze.usda_zip VALUES ('d', 'https://example.org/d.zip', '{}', 1);
             INSERT INTO bronze.record VALUES ('nutrients.toml', 'synthetic');
             CREATE TABLE bronze.usda_food_category (dataset VARCHAR, id VARCHAR, code VARCHAR, description VARCHAR);
             INSERT INTO bronze.usda_food_category VALUES ('d', '1', '0100', 'Vegetables');
             CREATE TABLE bronze.usda_food (dataset VARCHAR, fdc_id VARCHAR, data_type VARCHAR, description VARCHAR, food_category_id VARCHAR);
             INSERT INTO bronze.usda_food VALUES
               ('d', '1', 'sr_legacy_food', 'A', '1'), ('d', '2', 'foundation_food', 'B', '1'),
               ('d', '3', 'sample_food', 'C', '1');
             CREATE TABLE bronze.usda_nutrient (dataset VARCHAR, id VARCHAR, name VARCHAR, unit_name VARCHAR, nutrient_nbr VARCHAR);
             INSERT INTO bronze.usda_nutrient VALUES
               ('d', '1008', 'Energy', 'KCAL', '208'), ('d', '2048', 'Energy (Atwater)', 'KCAL', '958'),
               ('d', '2000', 'Sugars, Total', 'G', '269'), ('d', '1063', 'Sugars, NLEA', 'G', '269.3'),
               ('d', '1062', 'Energy', 'kJ', '268'), ('d', '1024', 'Specific Gravity', 'SP_GR', NULL);
             CREATE TABLE bronze.usda_food_nutrient (dataset VARCHAR, id VARCHAR, fdc_id VARCHAR, nutrient_id VARCHAR, amount VARCHAR);
             INSERT INTO bronze.usda_food_nutrient VALUES
               ('d', '1', '1', '1008', '100'), ('d', '2', '1', '2048', '90'), ('d', '3', '2', '2048', '50'),
               ('d', '4', '2', '1063', '7'), ('d', '5', '1', '2000', '3'), ('d', '6', '1', '2000', '3'),
               ('d', '7', '2', '2000', '1'), ('d', '8', '2', '2000', '2'), ('d', '9', '1', '9999', '5'),
               ('d', '10', '1', '1008', NULL), ('d', '11', '3', '1008', '1');",
            "a".repeat(64)
        ))
        .map_err(sql)?;
        load_silver(&conn, &Nutrients::parse(nutrients)?)?;
        Ok(conn)
    }

    fn strings(conn: &Connection, query: &str) -> Vec<String> {
        rows(conn, query, |r| r.get::<_, String>(0)).unwrap()
    }

    #[test]
    fn silver_keeps_a_row_or_rejects_it_with_its_reason() {
        let w = synthetic(SYNTHETIC_NUTRIENTS).unwrap();
        assert_eq!(
            strings(
                &w,
                "SELECT source_id FROM silver.usda_food_nutrient ORDER BY CAST(source_id AS INTEGER)"
            ),
            ["1", "2", "3", "4", "5"]
        );
        assert_eq!(
            strings(
                &w,
                "SELECT id || ': ' || reason FROM silver.usda_rejected ORDER BY CAST(id AS INTEGER)"
            ),
            [
                "6: repeats row 5: same food, nutrient and amount",
                "7: another row gives food 2 a different amount of nutrient 2000",
                "8: another row gives food 2 a different amount of nutrient 2000",
                "9: nutrient 9999 is not in its dataset's nutrient.csv",
                "10: no amount",
            ]
        );
        assert_eq!(
            strings(
                &w,
                "SELECT description FROM silver.usda_food ORDER BY fdc_id"
            ),
            ["A", "B"]
        );
    }

    #[test]
    fn the_first_listed_code_with_a_value_is_used() {
        let w = synthetic(SYNTHETIC_NUTRIENTS).unwrap();
        assert_eq!(
            strings(
                &w,
                "SELECT fdc_id || ' ' || nutrient || ' ' || amount || ' ' || code FROM gold.food_nutrient ORDER BY fdc_id, nutrient"
            ),
            [
                "1 energy 100.0 208",
                "1 sugars 3.0 269",
                "2 energy 50.0 958",
                "2 sugars 7.0 269.3"
            ]
        );
    }

    /// 269.3 is not 269: with only 269 listed, food 2 (which has sugars by 269.3, and only
    /// disagreeing rows by 269) has no sugars at all.
    #[test]
    fn codes_are_matched_exactly_not_rounded() {
        let only_269 = SYNTHETIC_NUTRIENTS.replace("[\"269\", \"269.3\"]", "[\"269\"]");
        let w = synthetic(&only_269).unwrap();
        assert_eq!(
            strings(
                &w,
                "SELECT fdc_id || ' ' || code FROM gold.food_nutrient WHERE nutrient = 'sugars'"
            ),
            ["1 269"]
        );
    }

    #[test]
    fn a_nutrient_code_usda_does_not_have_is_refused() {
        for code in ["2690", "269.30", "269.0"] {
            let wrong = SYNTHETIC_NUTRIENTS.replace("\"269.3\"", &format!("\"{code}\""));
            let Err(err) = synthetic(&wrong) else {
                panic!("loaded");
            };
            assert!(
                err.contains(&format!("code {code} is not in d's nutrient.csv")),
                "{err}"
            );
        }
    }

    #[test]
    fn a_nutrient_whose_unit_differs_is_refused() {
        let wrong = SYNTHETIC_NUTRIENTS.replace("unit = \"KCAL\"", "unit = \"kJ\"");
        let Err(err) = synthetic(&wrong) else {
            panic!("loaded");
        };
        assert!(
            err.contains("energy's code 208 has unit KCAL in d, not kJ"),
            "{err}"
        );
    }

    /// Silver's keys refuse a second food with one fdc_id, a food in no category, an amount for a
    /// nutrient its dataset lacks or a second amount for one food and nutrient, and one code in two
    /// nutrients; the control rows beside each are accepted.
    #[test]
    fn the_database_refuses_what_silver_s_keys_refuse() {
        let w = synthetic(SYNTHETIC_NUTRIENTS).unwrap();
        let accepted = |sql: &str| {
            w.execute_batch(&format!("BEGIN; {sql}; ROLLBACK;"))
                .map_err(|e| {
                    let _ = w.execute_batch("ROLLBACK");
                    e.to_string()
                })
        };
        let cases = [
            (
                "INSERT INTO silver.usda_food VALUES (9, 'd', 'sr_legacy_food', 'X', 1)",
                true,
            ),
            (
                "INSERT INTO silver.usda_food VALUES (1, 'd', 'sr_legacy_food', 'X', 1)",
                false,
            ),
            (
                "INSERT INTO silver.usda_food VALUES (9, 'd', 'sr_legacy_food', 'X', 7)",
                false,
            ),
            (
                "INSERT INTO silver.usda_food VALUES (9, 'd', 'sample_food', 'X', 1)",
                false,
            ),
            (
                "INSERT INTO silver.usda_food_nutrient VALUES (2, 'd', 1008, 1.0, 'x')",
                true,
            ),
            (
                "INSERT INTO silver.usda_food_nutrient VALUES (2, 'd', 9999, 1.0, 'x')",
                false,
            ),
            (
                "INSERT INTO silver.usda_food_nutrient VALUES (1, 'd', 1008, 1.0, 'x')",
                false,
            ),
            (
                "INSERT INTO silver.nutrient_code VALUES ('energy', 3, '268')",
                true,
            ),
            (
                "INSERT INTO silver.nutrient_code VALUES ('energy', 3, '269')",
                false,
            ),
            (
                "INSERT INTO silver.usda_rejected VALUES ('t', 'd', 'x', ' ')",
                false,
            ),
        ];
        for (sql, ok) in cases {
            assert_eq!(accepted(sql).is_ok(), ok, "{sql}: {:?}", accepted(sql));
        }
    }

    /// The warehouse, loaded once from the repo's records and the cached zips.
    fn warehouse() -> MutexGuard<'static, Connection> {
        static WAREHOUSE: OnceLock<Mutex<Connection>> = OnceLock::new();
        WAREHOUSE
            .get_or_init(|| {
                let cache = cache_dir().unwrap();
                Mutex::new(load(&repo_root(), &cache).unwrap_or_else(|e| panic!("{e}")))
            })
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn count(conn: &Connection, query: &str) -> i64 {
        conn.query_row(query, [], |r| r.get(0))
            .unwrap_or_else(|e| panic!("{query}: {e}"))
    }

    #[test]
    fn the_sources_record_is_in_bronze_as_read() {
        let w = warehouse();
        let body: String = w
            .query_row(
                "SELECT body FROM bronze.record WHERE path = 'sources.toml'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            body,
            fs::read_to_string(repo_root().join("sources.toml")).unwrap()
        );
    }

    /// The Done of plan step 3: SR Legacy ships its own count of each table's rows.
    #[test]
    fn the_sr_legacy_row_counts_equal_usda_s_own() {
        let w = warehouse();
        let mut stmt = w
            .prepare(
                "SELECT \"Table\", \"Number of Records\" FROM bronze.usda_all_downloaded_table_record_counts WHERE dataset = 'sr-legacy'",
            )
            .unwrap();
        let counts: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(counts.len() >= 9, "{counts:?}");
        for (table, n) in counts {
            let got = count(
                &w,
                &format!("SELECT count(*) FROM bronze.usda_{table} WHERE dataset = 'sr-legacy'"),
            );
            assert_eq!(got.to_string(), n, "bronze.usda_{table}");
        }
    }

    /// The number of records in a CSV, header excluded, counted apart from DuckDB: a line break
    /// inside double quotes does not end a record, and a final line without a break still counts.
    fn csv_records(bytes: &[u8]) -> i64 {
        let (mut lines, mut quoted) = (0, false);
        for &b in bytes {
            match b {
                b'"' => quoted = !quoted,
                b'\n' if !quoted => lines += 1,
                _ => {}
            }
        }
        if bytes.last().is_some_and(|&b| b != b'\n') {
            lines += 1;
        }
        lines - 1
    }

    #[test]
    fn what_counts_as_a_csv_record() {
        assert_eq!(csv_records(b"a,b\n1,2\n3,4\n"), 2);
        assert_eq!(csv_records(b"a,b\n1,2\n3,4"), 2);
        assert_eq!(csv_records(b"a,b\n\"1\n1\",2\n"), 1);
        assert_eq!(csv_records(b"a,b\n\"say \"\"hi\"\"\",2\n"), 1);
        assert_eq!(csv_records(b"a,b\n"), 0);
    }

    #[test]
    fn every_csv_in_the_pinned_zips_is_in_bronze_row_for_row() {
        let w = warehouse();
        let (sources, _) = Sources::load(&repo_root()).unwrap();
        let cache = cache_dir().unwrap();
        let scratch = Scratch::new().unwrap();
        let mut csvs = 0;
        for d in &sources.dataset {
            for m in usda::extract(&cache, d, &scratch.0).unwrap() {
                let loaded_as: Option<String> = w
                    .query_row(
                        "SELECT loaded_as FROM bronze.usda_member WHERE dataset = ? AND member = ?",
                        params![d.id, m.name],
                        |r| r.get(0),
                    )
                    .unwrap_or_else(|e| {
                        panic!("{} {}: not in bronze.usda_member: {e}", d.id, m.name)
                    });
                let Some(table) = loaded_as else {
                    assert!(
                        !m.name.ends_with(".csv"),
                        "{} {} was not loaded",
                        d.id,
                        m.name
                    );
                    continue;
                };
                csvs += 1;
                let rows = count(
                    &w,
                    &format!(
                        "SELECT count(*) FROM bronze.{table} WHERE dataset = {}",
                        literal(&d.id)
                    ),
                );
                assert_eq!(
                    rows,
                    csv_records(&fs::read(&m.file).unwrap()),
                    "{} {}: bronze.{table} has {rows} rows",
                    d.id,
                    m.name
                );
            }
        }
        assert!(csvs >= 30, "found {csvs} CSVs");
    }

    /// bronze.usda_member gives each file the length and sha256 of the bytes extracted from the
    /// pinned zip, not some other file's.
    #[test]
    fn every_file_in_the_pinned_zips_is_in_bronze_with_its_sha256() {
        let w = warehouse();
        let (sources, _) = Sources::load(&repo_root()).unwrap();
        let cache = cache_dir().unwrap();
        let scratch = Scratch::new().unwrap();
        let mut files = 0;
        for d in &sources.dataset {
            for m in usda::extract(&cache, d, &scratch.0).unwrap() {
                let (bytes, sha256): (i64, String) = w
                    .query_row(
                        "SELECT bytes, sha256 FROM bronze.usda_member WHERE dataset = ? AND member = ?",
                        params![d.id, m.name],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .unwrap_or_else(|e| panic!("{} {}: {e}", d.id, m.name));
                let content = fs::read(&m.file).unwrap();
                assert_eq!(bytes as usize, content.len(), "{} {}", d.id, m.name);
                assert_eq!(sha256, usda::sha256_hex(&content), "{} {}", d.id, m.name);
                files += 1;
            }
            let listed = count(
                &w,
                &format!(
                    "SELECT count(*) FROM bronze.usda_member WHERE dataset = {}",
                    literal(&d.id)
                ),
            );
            assert_eq!(
                listed as usize, files,
                "{}: files listed but not in its zip",
                d.id
            );
            files = 0;
        }
    }

    #[test]
    fn silver_holds_every_sr_legacy_and_foundation_food() {
        let w = warehouse();
        let expected = count(
            &w,
            "SELECT count(*) FROM bronze.usda_food WHERE data_type IN ('sr_legacy_food', 'foundation_food')",
        );
        assert!(expected >= 8000, "{expected}");
        assert_eq!(count(&w, "SELECT count(*) FROM silver.usda_food"), expected);
        assert_eq!(
            count(
                &w,
                "SELECT count(*) FROM silver.usda_food s JOIN bronze.usda_food b \
                 ON CAST(b.fdc_id AS INTEGER) = s.fdc_id AND b.dataset = s.dataset \
                 AND b.description = s.description AND b.data_type = s.data_type"
            ),
            expected
        );
    }

    /// Every food_nutrient row of a silver food is kept or rejected, never both and never neither.
    #[test]
    fn every_food_nutrient_row_in_scope_is_kept_or_rejected() {
        let w = warehouse();
        let in_scope = "SELECT dataset, id FROM bronze.usda_food_nutrient \
                        WHERE CAST(fdc_id AS INTEGER) IN (SELECT fdc_id FROM silver.usda_food)";
        let kept = "SELECT dataset, source_id FROM silver.usda_food_nutrient";
        let rejected =
            "SELECT dataset, id FROM silver.usda_rejected WHERE source = 'usda_food_nutrient'";
        let n = |q: &str| count(&w, &format!("SELECT count(*) FROM ({q})"));
        assert!(n(in_scope) >= 600_000);
        assert_eq!(n(&format!("{kept} UNION ALL {rejected}")), n(in_scope));
        assert_eq!(
            n(&format!("{in_scope} EXCEPT ({kept} UNION {rejected})")),
            0
        );
        assert_eq!(n(&format!("{kept} INTERSECT {rejected}")), 0);
    }

    /// A silver row names a dataset bronze.usda_zip holds, or a record bronze.record holds: the
    /// link DuckDB will not hold as a foreign key across schemas.
    #[test]
    fn every_silver_row_traces_to_the_bytes_it_came_from() {
        let w = warehouse();
        for table in [
            "usda_category",
            "usda_food",
            "usda_nutrient",
            "usda_food_nutrient",
            "usda_rejected",
        ] {
            let untraced = count(
                &w,
                &format!(
                    "SELECT count(*) FROM silver.{table} WHERE dataset NOT IN (SELECT dataset FROM bronze.usda_zip)"
                ),
            );
            assert_eq!(untraced, 0, "silver.{table}");
        }
        assert_eq!(
            count(
                &w,
                "SELECT count(*) FROM silver.usda_food_nutrient s WHERE NOT EXISTS \
                 (SELECT 1 FROM bronze.usda_food_nutrient b WHERE b.dataset = s.dataset AND b.id = s.source_id)"
            ),
            0
        );
        assert_eq!(
            count(
                &w,
                "SELECT count(*) FROM silver.nutrient WHERE record NOT IN (SELECT path FROM bronze.record)"
            ),
            0
        );
        assert!(count(&w, "SELECT count(*) FROM silver.nutrient") >= 16);
    }

    #[test]
    fn the_coverage_report_is_current() {
        let w = warehouse();
        let (sources, _) = Sources::load(&repo_root()).unwrap();
        let written = fs::read_to_string(repo_root().join(COVERAGE_PATH)).unwrap_or_default();
        assert!(
            written == coverage_report(&w, &sources).unwrap(),
            "{COVERAGE_PATH} is not what the warehouse writes: run `cargo run -p balaena-warehouse -- coverage`"
        );
    }

    #[test]
    fn the_catalogue_is_current() {
        let w = warehouse();
        let written = fs::read_to_string(repo_root().join(CATALOGUE_PATH)).unwrap_or_default();
        assert!(
            written == catalogue(&w).unwrap(),
            "{CATALOGUE_PATH} is not what the warehouse writes: run `cargo run -p balaena-warehouse -- catalogue`"
        );
    }

    #[test]
    fn every_table_and_view_says_what_it_is() {
        let w = warehouse();
        let silent = strings(
            &w,
            "SELECT schema_name || '.' || table_name FROM duckdb_tables() \
             WHERE schema_name IN ('bronze', 'silver', 'gold') AND coalesce(trim(comment), '') = '' \
             UNION ALL \
             SELECT schema_name || '.' || view_name FROM duckdb_views() \
             WHERE schema_name IN ('bronze', 'silver', 'gold') AND coalesce(trim(comment), '') = ''",
        );
        assert!(silent.is_empty(), "no comment: {silent:?}");
        let described = count(
            &w,
            "SELECT count(*) FROM duckdb_tables() WHERE schema_name IN ('bronze', 'silver', 'gold')",
        );
        assert!(described >= 35, "{described}");
    }

    #[test]
    fn every_usda_row_traces_to_the_bytes_it_came_from() {
        let w = warehouse();
        let mut stmt = w
            .prepare(
                "SELECT table_name FROM information_schema.tables WHERE table_schema = 'bronze' AND table_name LIKE 'usda\\_%' ESCAPE '\\' AND table_name NOT IN ('usda_zip', 'usda_member')",
            )
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(tables.len() >= 20, "{tables:?}");
        for table in tables {
            let rows = count(&w, &format!("SELECT count(*) FROM bronze.{table}"));
            let traced = count(
                &w,
                &format!(
                    "SELECT count(*) FROM bronze.{table} t JOIN bronze.usda_member m \
                     ON m.dataset = t.dataset AND m.loaded_as = {} \
                     JOIN bronze.usda_zip z ON z.dataset = m.dataset",
                    literal(&table)
                ),
            );
            assert_eq!(
                traced, rows,
                "bronze.{table}: rows that name no member and zip"
            );
        }
    }
}
