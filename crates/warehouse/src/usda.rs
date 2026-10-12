//! USDA's zips: fetched into the cache under their sha256, and refused wherever their bytes differ
//! from the pin in `sources.toml`.

use balaena_records::Dataset;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The sha256 of `bytes`, as 64 lowercase hex digits.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Where a dataset's zip lives in the cache: named by its sha256, so the name is its identity.
pub fn zip_path(cache: &Path, d: &Dataset) -> PathBuf {
    cache.join(format!("{}.zip", d.sha256))
}

/// Refuses bytes that are not the dataset's pinned zip: first their length, then their sha256.
pub fn verify(d: &Dataset, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 != d.bytes {
        return Err(format!(
            "{}: {} bytes, but sources.toml pins {}",
            d.id,
            bytes.len(),
            d.bytes
        ));
    }
    let got = sha256_hex(bytes);
    if got != d.sha256 {
        return Err(format!(
            "{}: sha256 {got}, but sources.toml pins {}",
            d.id, d.sha256
        ));
    }
    Ok(())
}

/// What `fetch` did with one dataset.
#[derive(Debug, PartialEq, Eq)]
pub enum Fetched {
    /// The zip was in the cache already, and matched its pin.
    Cached,
    /// The zip was downloaded, matched its pin, and was written to the cache.
    Downloaded,
}

/// Puts each dataset's zip in the cache, calling `get(url, limit)` for one that is not there. A zip
/// that differs from its pin is refused: a download is not written, and a cached zip is left as it
/// is for a person to look at, not replaced.
pub fn fetch(
    datasets: &[Dataset],
    cache: &Path,
    get: impl Fn(&str, u64) -> Result<Vec<u8>, String>,
) -> Result<Vec<(String, Fetched)>, String> {
    fs::create_dir_all(cache).map_err(|e| format!("{}: {e}", cache.display()))?;
    let mut out = Vec::new();
    for d in datasets {
        let path = zip_path(cache, d);
        if path.exists() {
            let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            verify(d, &bytes).map_err(|e| format!("{e} (the zip cached at {})", path.display()))?;
            out.push((d.id.clone(), Fetched::Cached));
            continue;
        }
        let bytes = get(&d.url, d.bytes)?;
        verify(d, &bytes).map_err(|e| format!("{e} (downloaded from {})", d.url))?;
        let part = path.with_extension("zip.part");
        fs::write(&part, &bytes).map_err(|e| format!("{}: {e}", part.display()))?;
        fs::rename(&part, &path).map_err(|e| format!("{}: {e}", path.display()))?;
        out.push((d.id.clone(), Fetched::Downloaded));
    }
    Ok(out)
}

/// How long one download may take, from the first lookup to the last byte, before it is abandoned.
/// ureq sets no limit of its own. On 2026-10-11, fetch downloaded both zips in 37.6 s.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// Downloads `url`, refusing a body longer than `limit` bytes, and abandoning a download that takes
/// longer than `DOWNLOAD_TIMEOUT`. ureq refuses a body that reaches its own limit (measured on
/// 2026-10-11: a limit of 6074592 refused a 6074592-byte body), so it is given one byte more.
pub fn http_get(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .build()
        .new_agent();
    let mut response = agent.get(url).call().map_err(|e| format!("{url}: {e}"))?;
    response
        .body_mut()
        .with_config()
        .limit(limit + 1)
        .read_to_vec()
        .map_err(|e| format!("{url}: {e}"))
}

/// One file in a dataset's zip, extracted.
#[derive(Debug)]
pub struct Member {
    /// The file's path inside the zip.
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    /// Where it was extracted to.
    pub file: PathBuf,
}

/// Reads the dataset's zip from the cache, refuses it if it differs from its pin, and extracts every
/// file in it under `dir/<dataset id>/`.
pub fn extract(cache: &Path, d: &Dataset, dir: &Path) -> Result<Vec<Member>, String> {
    let path = zip_path(cache, d);
    let bytes = fs::read(&path).map_err(|e| {
        format!(
            "{}: {e}; fetch it with `cargo run -p balaena-warehouse -- fetch`",
            path.display()
        )
    })?;
    verify(d, &bytes).map_err(|e| format!("{e} (the zip cached at {})", path.display()))?;
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("{}: {e}", d.id))?;
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("{}: {e}", d.id))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry
            .name()
            .map_err(|e| format!("{}: entry {i}: {e}", d.id))?
            .into_owned();
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| format!("{}: {name:?} would extract outside its folder", d.id))?;
        let mut content = Vec::new();
        entry
            .read_to_end(&mut content)
            .map_err(|e| format!("{}: {name}: {e}", d.id))?;
        let file = dir.join(&d.id).join(relative);
        fs::create_dir_all(file.parent().unwrap())
            .map_err(|e| format!("{}: {e}", file.display()))?;
        fs::write(&file, &content).map_err(|e| format!("{}: {e}", file.display()))?;
        out.push(Member {
            name,
            bytes: content.len() as u64,
            sha256: sha256_hex(&content),
            file,
        });
    }
    Ok(out)
}
