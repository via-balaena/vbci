//! `sources.toml`: the datasets the warehouse loads, each pinned by its zip's sha256 and length;
//! the Wikidata items it reads, each pinned by one revision's stored content; and the publishers
//! whose terms they come under.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// The record, as `sources.toml` holds it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sources {
    pub publisher: BTreeMap<String, Publisher>,
    pub dataset: Vec<Dataset>,
    /// Each comes under `[publisher.wikidata]`.
    #[serde(default)]
    pub wikidata: Vec<WikidataItem>,
}

/// Whose data a dataset is, and the terms it comes under.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Publisher {
    pub name: String,
    /// An SPDX license id, such as `CC0-1.0`.
    pub license: String,
    /// The page the terms are quoted from.
    pub terms_url: String,
    /// The UTC day the terms were read, `YYYY-MM-DD`.
    pub terms_read: String,
    /// The terms, quoted from `terms_url`.
    pub terms: String,
    /// The citation the publisher asks for, or None where its terms ask for none.
    pub citation: Option<String>,
}

/// One zip the warehouse loads, pinned by its sha256 and length.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
    /// Lowercase letters, digits and hyphens; unique in the record.
    pub id: String,
    /// A key of `[publisher]`.
    pub publisher: String,
    pub name: String,
    pub release: String,
    /// Where the zip is downloaded from, over https. Not its identity: `sha256` is.
    pub url: String,
    /// The zip's sha256, as 64 lowercase hex digits.
    pub sha256: String,
    /// The zip's length in bytes.
    pub bytes: u64,
}

/// One revision of a Wikidata item, pinned by the sha256 and length of the content Wikidata stores
/// for it: the item's JSON, as the revisions API gives it. A revision is never edited, so its
/// content is its identity, as a zip's bytes are a dataset's.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WikidataItem {
    /// `Q<digits>`; unique in the record.
    pub item: String,
    /// The revision's id, above 0.
    pub revision: u64,
    /// The content's sha256, as 64 lowercase hex digits.
    pub sha256: String,
    /// The content's length in bytes.
    pub bytes: u64,
}

/// Whether `s` is a sha256 as `sources.toml` writes one: 64 lowercase hex digits.
fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

impl Sources {
    /// The record's path, relative to the repo root.
    pub const PATH: &'static str = "sources.toml";

    /// Reads `sources.toml` under `root`, returning the record and the text it was parsed from.
    pub fn load(root: &Path) -> Result<(Sources, String), String> {
        let path = root.join(Self::PATH);
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let sources = Self::parse(&text).map_err(|e| format!("{}: {e}", Self::PATH))?;
        Ok((sources, text))
    }

    /// Parses the record and checks the rules its comments state.
    pub fn parse(text: &str) -> Result<Sources, String> {
        let sources: Sources = toml::from_str(text).map_err(|e| e.to_string())?;
        sources.check()?;
        Ok(sources)
    }

    fn check(&self) -> Result<(), String> {
        for (key, p) in &self.publisher {
            for (field, value) in [
                ("name", Some(&p.name)),
                ("license", Some(&p.license)),
                ("terms", Some(&p.terms)),
                ("citation", p.citation.as_ref()),
            ] {
                if value.is_some_and(|v| v.trim().is_empty()) {
                    return Err(format!("publisher {key}: {field} is empty"));
                }
            }
            if !p.terms_url.starts_with("https://") {
                return Err(format!("publisher {key}: terms_url is not https"));
            }
            if !is_day(&p.terms_read) {
                return Err(format!(
                    "publisher {key}: terms_read {:?} is not a day, YYYY-MM-DD",
                    p.terms_read
                ));
            }
        }
        if self.dataset.is_empty() {
            return Err("no dataset".into());
        }
        let mut ids = BTreeSet::new();
        for d in &self.dataset {
            let id = &d.id;
            let id_ok = !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !id_ok {
                return Err(format!(
                    "dataset {id:?}: an id is lowercase letters, digits and hyphens"
                ));
            }
            if !ids.insert(id.as_str()) {
                return Err(format!("dataset {id}: two datasets have this id"));
            }
            if !self.publisher.contains_key(&d.publisher) {
                return Err(format!(
                    "dataset {id}: publisher {:?} is not in [publisher]",
                    d.publisher
                ));
            }
            for (field, value) in [("name", &d.name), ("release", &d.release)] {
                if value.trim().is_empty() {
                    return Err(format!("dataset {id}: {field} is empty"));
                }
            }
            if !d.url.starts_with("https://") {
                return Err(format!("dataset {id}: url is not https"));
            }
            if !is_sha256(&d.sha256) {
                return Err(format!(
                    "dataset {id}: sha256 is not 64 lowercase hex digits"
                ));
            }
            if d.bytes == 0 {
                return Err(format!("dataset {id}: bytes is 0"));
            }
        }
        if !self.wikidata.is_empty() && !self.publisher.contains_key(WIKIDATA) {
            return Err(format!(
                "a [[wikidata]] item comes under [publisher.{WIKIDATA}], which the record lacks"
            ));
        }
        let mut items = BTreeSet::new();
        for w in &self.wikidata {
            let item = &w.item;
            if !crate::is_wikidata_item(item) {
                return Err(format!("wikidata {item:?}: an item is Q<digits>"));
            }
            if !items.insert(item.as_str()) {
                return Err(format!("wikidata {item}: listed twice"));
            }
            if w.revision == 0 {
                return Err(format!("wikidata {item}: revision is 0"));
            }
            if !is_sha256(&w.sha256) {
                return Err(format!(
                    "wikidata {item}: sha256 is not 64 lowercase hex digits"
                ));
            }
            if w.bytes == 0 {
                return Err(format!("wikidata {item}: bytes is 0"));
            }
        }
        Ok(())
    }
}

/// The publisher every `[[wikidata]]` item comes under.
pub const WIKIDATA: &str = "wikidata";

/// Whether `s` is a day written `YYYY-MM-DD`, with a month 01-12 and a day 01-31.
fn is_day(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    let digits = |p: &str, n: usize| p.len() == n && p.chars().all(|c| c.is_ascii_digit());
    if parts.len() != 3 || !digits(parts[0], 4) || !digits(parts[1], 2) || !digits(parts[2], 2) {
        return false;
    }
    let (month, day): (u32, u32) = (parts[1].parse().unwrap(), parts[2].parse().unwrap());
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
[publisher.p]
name = "P"
license = "CC0-1.0"
terms_url = "https://example.org/terms"
terms_read = "2026-10-11"
terms = "Free."
citation = "P, 2026."

[[dataset]]
id = "d-1"
publisher = "p"
name = "D"
release = "2026-04"
url = "https://example.org/d.zip"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
bytes = 10
"#;

    /// `GOOD` with one replacement, which must occur exactly once.
    fn with(old: &str, new: &str) -> String {
        assert_eq!(GOOD.matches(old).count(), 1, "{old:?}");
        GOOD.replace(old, new)
    }

    fn refused(text: &str, why: &str) {
        let err = Sources::parse(text).expect_err("refused");
        assert!(err.contains(why), "{err:?} does not say {why:?}");
    }

    /// Wikidata's publisher, which asks for no citation, and one item under it.
    const WIKIDATA_ITEM: &str = r#"
[publisher.wikidata]
name = "Wikidata"
license = "CC0-1.0"
terms_url = "https://www.wikidata.org/wiki/Wikidata:Licensing"
terms_read = "2026-10-16"
terms = "Public domain."

[[wikidata]]
item = "Q500"
revision = 7
sha256 = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
bytes = 20
"#;

    /// `GOOD` and `WIKIDATA_ITEM`, with one replacement in the latter, which must occur exactly
    /// once.
    fn with_item(old: &str, new: &str) -> String {
        assert_eq!(WIKIDATA_ITEM.matches(old).count(), 1, "{old:?}");
        format!("{GOOD}{}", WIKIDATA_ITEM.replace(old, new))
    }

    #[test]
    fn a_good_record_loads() {
        let s = Sources::parse(GOOD).unwrap();
        assert_eq!(s.dataset[0].bytes, 10);
        assert_eq!(s.publisher["p"].license, "CC0-1.0");
        assert!(s.wikidata.is_empty());
        let s = Sources::parse(&with_item("bytes = 20", "bytes = 20")).unwrap();
        assert_eq!(
            (s.wikidata[0].item.as_str(), s.wikidata[0].revision),
            ("Q500", 7)
        );
        assert_eq!(s.publisher["wikidata"].citation, None);
    }

    /// A publisher whose terms ask for no citation has none; one that has a citation does not
    /// leave it empty.
    #[test]
    fn a_publisher_s_citation_is_one_it_asks_for() {
        assert_eq!(
            Sources::parse(GOOD).unwrap().publisher["p"]
                .citation
                .as_deref(),
            Some("P, 2026.")
        );
        refused(
            &with("citation = \"P, 2026.\"", "citation = \" \""),
            "publisher p: citation is empty",
        );
    }

    #[test]
    fn a_wikidata_item_is_pinned_by_its_revision_s_sha256_and_length() {
        let sha = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        refused(
            &with_item(sha, &sha.to_uppercase()),
            "wikidata Q500: sha256 is not 64",
        );
        refused(
            &with_item(sha, &sha[1..]),
            "wikidata Q500: sha256 is not 64",
        );
        refused(
            &with_item("bytes = 20", "bytes = 0"),
            "wikidata Q500: bytes is 0",
        );
        refused(
            &with_item("revision = 7", "revision = 0"),
            "wikidata Q500: revision is 0",
        );
        refused(&with_item("revision = 7\n", ""), "missing field `revision`");
        refused(
            &with_item("bytes = 20", "bytes = 20\nurl = \"https://example.org\""),
            "unknown field `url`",
        );
    }

    #[test]
    fn a_wikidata_item_is_listed_once_under_wikidata_s_publisher() {
        for bad in ["q500", "500", "Q", "Q5x"] {
            refused(
                &with_item("item = \"Q500\"", &format!("item = \"{bad}\"")),
                "an item is Q<digits>",
            );
        }
        let second = &WIKIDATA_ITEM[WIKIDATA_ITEM.find("[[wikidata]]").unwrap()..];
        refused(
            &format!("{}{second}", with_item("bytes = 20", "bytes = 20")),
            "wikidata Q500: listed twice",
        );
        let alone = format!("{GOOD}{second}");
        refused(
            &alone,
            "comes under [publisher.wikidata], which the record lacks",
        );
    }

    #[test]
    fn a_source_record_refuses_a_key_it_does_not_know() {
        refused(&format!("extra = 1\n{GOOD}"), "unknown field `extra`");
        refused(
            &with("citation = ", "notes = \"x\"\ncitation = "),
            "unknown field `notes`",
        );
        refused(
            &with("bytes = 10", "bytes = 10\nmd5 = \"x\""),
            "unknown field `md5`",
        );
    }

    #[test]
    fn a_dataset_is_pinned_by_a_lowercase_sha256_and_a_length() {
        let sha = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        refused(&with(sha, &sha.to_uppercase()), "64 lowercase hex");
        refused(&with(sha, &sha[1..]), "64 lowercase hex");
        refused(&with(sha, &format!("{}g", &sha[1..])), "64 lowercase hex");
        refused(&with("bytes = 10", "bytes = 0"), "bytes is 0");
        refused(&with("bytes = 10\n", ""), "missing field `bytes`");
    }

    #[test]
    fn a_dataset_names_a_publisher_the_record_lists() {
        refused(
            &with("publisher = \"p\"", "publisher = \"q\""),
            "not in [publisher]",
        );
    }

    #[test]
    fn dataset_ids_are_unique_and_plain() {
        let second = &GOOD[GOOD.find("[[dataset]]").unwrap()..];
        refused(&format!("{GOOD}\n{second}"), "two datasets have this id");
        refused(&with("id = \"d-1\"", "id = \"D 1\""), "lowercase letters");
        refused(&with("id = \"d-1\"", "id = \"\""), "lowercase letters");
    }

    #[test]
    fn a_dataset_and_its_terms_are_fetched_over_https() {
        refused(
            &with("https://example.org/d.zip", "http://example.org/d.zip"),
            "url is not https",
        );
        refused(
            &with("https://example.org/terms", "ftp://example.org/terms"),
            "terms_url is not https",
        );
    }

    #[test]
    fn a_publisher_says_when_its_terms_were_read() {
        refused(&with("2026-10-11", "2026-13-11"), "not a day");
        refused(&with("2026-10-11", "11 October 2026"), "not a day");
        refused(
            &with("terms = \"Free.\"", "terms = \" \""),
            "terms is empty",
        );
    }

    #[test]
    fn the_sources_record_loads() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let (sources, text) = Sources::load(&root).unwrap();
        assert!(sources.dataset.len() >= 2, "{text}");
        assert!(sources.dataset.iter().any(|d| d.id == "sr-legacy"));
        assert!(sources.wikidata.len() >= 2, "{text}");
    }
}
