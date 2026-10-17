//! Wikidata's items: each pinned revision's content, fetched into the cache under its sha256 and
//! refused wherever its bytes differ from the pin in `sources.toml`; the two properties an
//! ingredient's organism is read from; and how a taxon is read from them.

use crate::usda::{Fetched, sha256_hex};
use balaena_records::WikidataItem;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Wikidata's NCBI taxonomy ID: an NCBI Taxonomy id's digits.
pub const NCBI_TAXONOMY_ID: &str = "P685";

/// Wikidata's natural product of taxon: the taxon a product, such as a food, comes from.
pub const NATURAL_PRODUCT_OF_TAXON: &str = "P1582";

/// Where an item's pinned revision is fetched from: Wikidata's revisions API, whose answer carries
/// the content Wikidata stores for the revision. Its pages give no plain content for an item
/// (2026-10-16: `action=raw` answered 415, "content model `wikibase-item` ... not supported").
pub fn url(w: &WikidataItem) -> String {
    format!(
        "https://www.wikidata.org/w/api.php?action=query&prop=revisions&revids={}\
         &rvslots=main&rvprop=content%7Cids&format=json&formatversion=2",
        w.revision
    )
}

/// The most an answer may weigh: twice the content, as a JSON string escapes some of it, and
/// 4 KiB for the rest. On 2026-10-16 the four pinned revisions' answers were 1.17 to 1.20 times
/// their content.
pub fn answer_limit(w: &WikidataItem) -> u64 {
    2 * w.bytes + 4096
}

/// Where a revision's content lives in the cache: named by its sha256, so the name is its identity.
pub fn content_path(cache: &Path, w: &WikidataItem) -> PathBuf {
    cache.join(format!("{}.json", w.sha256))
}

/// Refuses bytes that are not the item's pinned content: first their length, then their sha256.
pub fn verify(w: &WikidataItem, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 != w.bytes {
        return Err(format!(
            "wikidata {}: {} bytes, but sources.toml pins {}",
            w.item,
            bytes.len(),
            w.bytes
        ));
    }
    let got = sha256_hex(bytes);
    if got != w.sha256 {
        return Err(format!(
            "wikidata {}: sha256 {got}, but sources.toml pins {}",
            w.item, w.sha256
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
struct Answer {
    query: Query,
}

#[derive(Deserialize)]
struct Query {
    #[serde(default)]
    pages: Vec<Page>,
}

#[derive(Deserialize)]
struct Page {
    title: String,
    #[serde(default)]
    revisions: Vec<Revision>,
}

#[derive(Deserialize)]
struct Revision {
    revid: u64,
    slots: Slots,
}

#[derive(Deserialize)]
struct Slots {
    main: Slot,
}

#[derive(Deserialize)]
struct Slot {
    content: String,
}

/// The content the revisions API's answer carries, refusing an answer about another item or
/// another revision: a revision id is unique across all of Wikidata, so a mistyped one finds
/// some other item's.
pub fn content_of(w: &WikidataItem, answer: &[u8]) -> Result<Vec<u8>, String> {
    let at = |e: String| format!("wikidata {} revision {}: {e}", w.item, w.revision);
    let a: Answer = serde_json::from_slice(answer).map_err(|e| at(e.to_string()))?;
    let [page] = &a.query.pages[..] else {
        return Err(at(format!(
            "the answer has {} pages, not one",
            a.query.pages.len()
        )));
    };
    if page.title != w.item {
        return Err(at(format!("the revision is {}'s", page.title)));
    }
    let [revision] = &page.revisions[..] else {
        return Err(at(format!(
            "the answer has {} revisions, not one",
            page.revisions.len()
        )));
    };
    if revision.revid != w.revision {
        return Err(at(format!("the answer is revision {}", revision.revid)));
    }
    Ok(revision.slots.main.content.clone().into_bytes())
}

/// Puts each item's pinned content in the cache, calling `get(url, limit)` for one that is not
/// there. Content that differs from its pin is refused: a download is not written, and cached
/// content is left as it is for a person to look at, not replaced.
pub fn fetch(
    items: &[WikidataItem],
    cache: &Path,
    get: impl Fn(&str, u64) -> Result<Vec<u8>, String>,
) -> Result<Vec<(String, Fetched)>, String> {
    fs::create_dir_all(cache).map_err(|e| format!("{}: {e}", cache.display()))?;
    let mut out = Vec::new();
    for w in items {
        let path = content_path(cache, w);
        if path.exists() {
            let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            verify(w, &bytes)
                .map_err(|e| format!("{e} (the content cached at {})", path.display()))?;
            out.push((w.item.clone(), Fetched::Cached));
            continue;
        }
        let url = url(w);
        let bytes = content_of(w, &get(&url, answer_limit(w))?)?;
        verify(w, &bytes).map_err(|e| format!("{e} (downloaded from {url})"))?;
        let part = path.with_extension("json.part");
        fs::write(&part, &bytes).map_err(|e| format!("{}: {e}", part.display()))?;
        fs::rename(&part, &path).map_err(|e| format!("{}: {e}", path.display()))?;
        out.push((w.item.clone(), Fetched::Downloaded));
    }
    Ok(out)
}

/// Reads the item's content from the cache, refusing it if it differs from its pin.
pub fn read(cache: &Path, w: &WikidataItem) -> Result<String, String> {
    let path = content_path(cache, w);
    let bytes = fs::read(&path).map_err(|e| {
        format!(
            "{}: {e}; fetch it with `cargo run -p balaena-warehouse -- fetch`",
            path.display()
        )
    })?;
    verify(w, &bytes).map_err(|e| format!("{e} (the content cached at {})", path.display()))?;
    String::from_utf8(bytes).map_err(|e| format!("wikidata {}: {e}", w.item))
}

/// One statement of a property read, as an item's content gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub property: &'static str,
    /// For P685 the id's digits, for P1582 the item it names; None where the statement says some
    /// value or no value rather than giving one.
    pub value: Option<String>,
    /// `preferred`, `normal` or `deprecated`.
    pub rank: String,
}

/// The statements of P685 and P1582 in an item's content, each property's in its order there.
/// Refuses content that is not `item`'s, or whose statements are not shaped as Wikidata's are.
pub fn statements(item: &str, content: &str) -> Result<Vec<Statement>, String> {
    let at = |e: &str| format!("wikidata {item}: {e}");
    let v: Value = serde_json::from_str(content).map_err(|e| at(&e.to_string()))?;
    if v["id"] != item {
        return Err(at(&format!("the content is {}'s", v["id"])));
    }
    let mut out = Vec::new();
    for property in [NCBI_TAXONOMY_ID, NATURAL_PRODUCT_OF_TAXON] {
        let Some(claims) = v["claims"].get(property) else {
            continue;
        };
        let claims = claims
            .as_array()
            .ok_or_else(|| at(&format!("{property} is not a list of statements")))?;
        for c in claims {
            let rank = c["rank"]
                .as_str()
                .ok_or_else(|| at(&format!("a {property} statement has no rank")))?;
            let snak = &c["mainsnak"];
            let value = match snak["snaktype"].as_str() {
                Some("value") => {
                    let v = &snak["datavalue"]["value"];
                    let text = if property == NCBI_TAXONOMY_ID {
                        v.as_str()
                    } else {
                        v["id"].as_str()
                    };
                    Some(
                        text.ok_or_else(|| at(&format!("a {property} value is {v}")))?
                            .to_string(),
                    )
                }
                Some("somevalue" | "novalue") => None,
                other => {
                    return Err(at(&format!(
                        "a {property} statement's snak type is {other:?}"
                    )));
                }
            };
            out.push(Statement {
                property,
                value,
                rank: rank.to_string(),
            });
        }
    }
    Ok(out)
}

/// The one value an item gives for `property`, None if it gives none. A deprecated statement is
/// not read; two read statements, or one with no value, are refused.
fn one(item: &str, statements: &[Statement], property: &str) -> Result<Option<String>, String> {
    let read: Vec<&Statement> = statements
        .iter()
        .filter(|s| s.property == property && s.rank != "deprecated")
        .collect();
    match read[..] {
        [] => Ok(None),
        [s] => s
            .value
            .clone()
            .map(Some)
            .ok_or_else(|| format!("Wikidata item {item}'s {property} statement has no value")),
        _ => Err(format!(
            "Wikidata item {item} has {} {property} statements, not one",
            read.len()
        )),
    }
}

/// The NCBI Taxonomy id an item gives, as `NCBITaxon_<digits>`, and the item it was read from: the
/// item's own P685, else the P685 of the one item its P1582 names. `pinned` holds each pinned
/// item's statements; an item it lacks is not pinned.
pub fn taxon(
    item: &str,
    pinned: &BTreeMap<String, Vec<Statement>>,
) -> Result<(String, String), String> {
    let statements = pinned
        .get(item)
        .ok_or_else(|| format!("Wikidata item {item} is not pinned in sources.toml"))?;
    if let Some(id) = one(item, statements, NCBI_TAXONOMY_ID)? {
        return Ok((format!("NCBITaxon_{id}"), item.to_string()));
    }
    let Some(product_of) = one(item, statements, NATURAL_PRODUCT_OF_TAXON)? else {
        return Err(format!(
            "Wikidata item {item} has no {NCBI_TAXONOMY_ID} and no {NATURAL_PRODUCT_OF_TAXON}"
        ));
    };
    let of = pinned.get(&product_of).ok_or_else(|| {
        format!(
            "Wikidata item {item}'s {NATURAL_PRODUCT_OF_TAXON} is {product_of}, which sources.toml does not pin"
        )
    })?;
    match one(&product_of, of, NCBI_TAXONOMY_ID)? {
        Some(id) => Ok((format!("NCBITaxon_{id}"), product_of)),
        None => Err(format!(
            "Wikidata item {product_of}, which {item}'s {NATURAL_PRODUCT_OF_TAXON} names, has no {NCBI_TAXONOMY_ID}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(property: &'static str, value: Option<&str>, rank: &str) -> Statement {
        Statement {
            property,
            value: value.map(str::to_string),
            rank: rank.to_string(),
        }
    }

    fn pinned(items: &[(&str, Vec<Statement>)]) -> BTreeMap<String, Vec<Statement>> {
        items
            .iter()
            .map(|(q, s)| (q.to_string(), s.clone()))
            .collect()
    }

    fn ncbi(id: &str) -> Statement {
        st(NCBI_TAXONOMY_ID, Some(id), "normal")
    }

    fn product_of(item: &str) -> Statement {
        st(NATURAL_PRODUCT_OF_TAXON, Some(item), "normal")
    }

    /// An item's own P685 gives its taxon, over its P1582; an item with none, or only a deprecated
    /// one, is read through the item its P1582 names.
    #[test]
    fn an_item_s_taxon_is_its_own_p685_else_its_product_of_s() {
        let p = pinned(&[
            ("Q1", vec![ncbi("1")]),
            ("Q2", vec![product_of("Q3"), ncbi("2")]),
            ("Q3", vec![ncbi("3")]),
            ("Q4", vec![product_of("Q3")]),
            (
                "Q5",
                vec![
                    st(NCBI_TAXONOMY_ID, Some("9"), "deprecated"),
                    product_of("Q3"),
                ],
            ),
            ("Q6", vec![st(NCBI_TAXONOMY_ID, Some("6"), "preferred")]),
        ]);
        let got = |q: &str| taxon(q, &p);
        assert_eq!(got("Q1"), Ok(("NCBITaxon_1".into(), "Q1".into())));
        assert_eq!(got("Q2"), Ok(("NCBITaxon_2".into(), "Q2".into())));
        assert_eq!(got("Q4"), Ok(("NCBITaxon_3".into(), "Q3".into())));
        assert_eq!(got("Q5"), Ok(("NCBITaxon_3".into(), "Q3".into())));
        assert_eq!(got("Q6"), Ok(("NCBITaxon_6".into(), "Q6".into())));
    }

    /// Each place a taxon could be read from more than one way, or not at all, is refused: an item
    /// not pinned, two read statements of a property, one with no value, neither property, a P1582
    /// naming an item not pinned, or one with no P685 of its own; one step only.
    #[test]
    fn an_item_whose_taxon_is_not_one_value_is_refused() {
        let p = pinned(&[
            ("Q1", vec![ncbi("1"), ncbi("2")]),
            (
                "Q2",
                vec![ncbi("1"), st(NCBI_TAXONOMY_ID, Some("2"), "preferred")],
            ),
            ("Q3", vec![st(NCBI_TAXONOMY_ID, None, "normal")]),
            ("Q4", vec![]),
            ("Q5", vec![product_of("Q99")]),
            ("Q6", vec![product_of("Q4")]),
            ("Q7", vec![product_of("Q6")]),
            ("Q8", vec![product_of("Q10"), product_of("Q11")]),
            ("Q9", vec![product_of("Q1")]),
            ("Q10", vec![ncbi("10")]),
            ("Q11", vec![ncbi("11")]),
        ]);
        for (item, why) in [
            ("Q0", "Wikidata item Q0 is not pinned in sources.toml"),
            ("Q1", "Wikidata item Q1 has 2 P685 statements, not one"),
            ("Q2", "Wikidata item Q2 has 2 P685 statements, not one"),
            ("Q3", "Wikidata item Q3's P685 statement has no value"),
            ("Q4", "Wikidata item Q4 has no P685 and no P1582"),
            (
                "Q5",
                "Wikidata item Q5's P1582 is Q99, which sources.toml does not pin",
            ),
            (
                "Q6",
                "Wikidata item Q4, which Q6's P1582 names, has no P685",
            ),
            (
                "Q7",
                "Wikidata item Q6, which Q7's P1582 names, has no P685",
            ),
            ("Q8", "Wikidata item Q8 has 2 P1582 statements, not one"),
            ("Q9", "Wikidata item Q1 has 2 P685 statements, not one"),
        ] {
            let err = taxon(item, &p).unwrap_err();
            assert_eq!(err, why, "{item}");
        }
    }

    /// An item's content, shaped as Q1375049's revision 2549623412 is, cut down to one P1582
    /// statement and a property not read (P31), with two P685 statements added: one preferred,
    /// one with some value and deprecated.
    const CONTENT: &str = r#"{"type":"item","id":"Q1","labels":{"en":{"language":"en","value":"lemon juice"}},"claims":{"P31":[{"mainsnak":{"snaktype":"value","property":"P31","datavalue":{"value":{"entity-type":"item","numeric-id":5,"id":"Q5"},"type":"wikibase-entityid"}},"type":"statement","rank":"normal"}],"P1582":[{"mainsnak":{"snaktype":"value","property":"P1582","hash":"x","datavalue":{"value":{"entity-type":"item","numeric-id":500,"id":"Q500"},"type":"wikibase-entityid"},"datatype":"wikibase-item"},"type":"statement","id":"Q1$a","rank":"normal","references":[]}],"P685":[{"mainsnak":{"snaktype":"value","property":"P685","datavalue":{"value":"2708","type":"string"}},"type":"statement","rank":"preferred"},{"mainsnak":{"snaktype":"somevalue","property":"P685"},"type":"statement","rank":"deprecated"}]},"sitelinks":{}}"#;

    #[test]
    fn statements_are_read_as_wikidata_writes_them() {
        assert_eq!(
            statements("Q1", CONTENT),
            Ok(vec![
                st(NCBI_TAXONOMY_ID, Some("2708"), "preferred"),
                st(NCBI_TAXONOMY_ID, None, "deprecated"),
                st(NATURAL_PRODUCT_OF_TAXON, Some("Q500"), "normal"),
            ])
        );
        assert_eq!(
            statements("Q1", r#"{"id":"Q1","claims":{}}"#),
            Ok(Vec::new())
        );
        for (content, why) in [
            (CONTENT.to_string(), "wikidata Q2: the content is \"Q1\"'s"),
            (
                CONTENT.replace("\"value\":\"2708\"", "\"value\":2708"),
                "wikidata Q1: a P685 value is 2708",
            ),
            (
                CONTENT.replace(
                    "\"value\":{\"entity-type\":\"item\",\"numeric-id\":500,\"id\":\"Q500\"}",
                    "\"value\":\"Q500\"",
                ),
                "wikidata Q1: a P1582 value is \"Q500\"",
            ),
            (
                CONTENT.replace("\"snaktype\":\"somevalue\"", "\"snaktype\":\"other\""),
                "wikidata Q1: a P685 statement's snak type is Some(\"other\")",
            ),
            (
                CONTENT.replace(",\"rank\":\"normal\",\"references\"", ",\"references\""),
                "wikidata Q1: a P1582 statement has no rank",
            ),
            (
                r#"{"id":"Q1","claims":{"P685":{}}}"#.to_string(),
                "wikidata Q1: P685 is not a list of statements",
            ),
        ] {
            let item = if why.starts_with("wikidata Q2") {
                "Q2"
            } else {
                "Q1"
            };
            assert_eq!(statements(item, &content), Err(why.to_string()));
        }
    }

    fn pin(item: &str, revision: u64) -> WikidataItem {
        WikidataItem {
            item: item.into(),
            revision,
            sha256: "0".repeat(64),
            bytes: 1,
        }
    }

    /// The answer the revisions API gives for one revision, as of 2026-10-16, cut down.
    fn answer(title: &str, revid: u64, content: &str) -> String {
        format!(
            r#"{{"batchcomplete":true,"query":{{"pages":[{{"pageid":1,"ns":0,"title":"{title}","revisions":[{{"revid":{revid},"parentid":1,"slots":{{"main":{{"contentmodel":"wikibase-item","contentformat":"application/json","content":{}}}}}}}]}}]}}}}"#,
            serde_json::to_string(content).unwrap()
        )
    }

    /// The content comes out of the answer as Wikidata stored it; an answer about another item,
    /// another revision, no page (an unknown revision) or two pages is refused.
    #[test]
    fn an_answer_about_another_item_or_revision_is_refused() {
        let w = pin("Q1", 7);
        assert_eq!(
            content_of(&w, answer("Q1", 7, CONTENT).as_bytes()),
            Ok(CONTENT.as_bytes().to_vec())
        );
        let unknown =
            r#"{"batchcomplete":true,"query":{"badrevids":{"7":{"revid":7,"missing":true}}}}"#;
        let two = answer("Q1", 7, "{}").replace("[{\"pageid\"", "[{\"title\":\"Q1\"},{\"pageid\"");
        for (body, why) in [
            (
                answer("Q2", 7, CONTENT),
                "wikidata Q1 revision 7: the revision is Q2's",
            ),
            (
                answer("Q1", 8, CONTENT),
                "wikidata Q1 revision 7: the answer is revision 8",
            ),
            (
                unknown.to_string(),
                "wikidata Q1 revision 7: the answer has 0 pages, not one",
            ),
            (
                two,
                "wikidata Q1 revision 7: the answer has 2 pages, not one",
            ),
        ] {
            assert_eq!(content_of(&w, body.as_bytes()), Err(why.to_string()));
        }
    }
}
