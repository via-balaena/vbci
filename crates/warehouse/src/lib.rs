//! VBCI's warehouse, in DuckDB: rebuilt from the records and USDA's pinned zips on every load, so it
//! can be deleted at any time. `schema.sql` describes its layers; bronze is the only one so far.

pub mod usda;

use balaena_records::Sources;
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
/// Bronze takes `sources.toml` as read, each dataset's zip as pinned, every file in each zip with
/// its sha256, and every CSV row for row, as text.
pub fn load(root: &Path, cache: &Path) -> Result<Connection, String> {
    let (sources, text) = Sources::load(root)?;
    let conn = Connection::open_in_memory().map_err(sql)?;
    conn.execute_batch(SCHEMA).map_err(sql)?;
    conn.execute(
        "INSERT INTO bronze.record VALUES (?, ?)",
        params![Sources::PATH, text],
    )
    .map_err(sql)?;
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
    Ok(conn)
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
