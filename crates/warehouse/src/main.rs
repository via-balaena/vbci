//! `cargo run -p balaena-warehouse -- <command>`
//!
//! - `fetch`: puts each zip `sources.toml` pins into the cache, downloading the ones not there, and
//!   refuses bytes that differ from their pin.
//! - `load`: builds the warehouse and prints each bronze table's row count.

use balaena_records::Sources;
use balaena_warehouse::{cache_dir, load, repo_root, usda};
use std::process::ExitCode;

fn run(command: &str) -> Result<(), String> {
    let root = repo_root();
    let cache = cache_dir()?;
    match command {
        "fetch" => {
            let (sources, _) = Sources::load(&root)?;
            for (id, how) in usda::fetch(&sources.dataset, &cache, usda::http_get)? {
                println!("{id}: {how:?}, in {}", cache.display());
            }
        }
        "load" => {
            let conn = load(&root, &cache)?;
            let mut stmt = conn
                .prepare(
                    "SELECT table_name FROM information_schema.tables WHERE table_schema = 'bronze' AND table_type = 'BASE TABLE' ORDER BY table_name",
                )
                .map_err(|e| e.to_string())?;
            let tables: Vec<String> = stmt
                .query_map([], |r| r.get(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?;
            for table in tables {
                let rows: i64 = conn
                    .query_row(&format!("SELECT count(*) FROM bronze.{table}"), [], |r| {
                        r.get(0)
                    })
                    .map_err(|e| e.to_string())?;
                println!("bronze.{table}\t{rows}");
            }
        }
        other => return Err(format!("unknown command {other:?}: fetch or load")),
    }
    Ok(())
}

fn main() -> ExitCode {
    let command = std::env::args().nth(1).unwrap_or_default();
    match run(&command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
