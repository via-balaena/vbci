//! VBCI's records, typed. Each schema refuses a key it doesn't know, and each loader checks the
//! rules its record's comments state. Parsing a record needs no database and no network.

pub mod crops;
pub mod diet;
pub mod ingredients;
pub mod nutrients;
pub mod sources;
pub mod units;

pub use crops::{Crop, Crops};
pub use diet::{Answer, Class, Diet, DietCategory, Exception, Organism, Rule, Taxon};
pub use ingredients::{Form, Ingredient, IngredientOrganism, IngredientOrigin, Ingredients, State};
pub use nutrients::{Nutrient, Nutrients};
pub use sources::{Dataset, Publisher, Sources, WikidataItem};
pub use units::{CountUnit, CountUnits};

/// Whether `s` is lowercase letters and digits joined by single hyphens: a record's id, as its
/// file is named.
fn is_id(s: &str) -> bool {
    !s.is_empty()
        && s.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// Every file in the folder `dir` under `root`, as (file name, text), sorted by name.
fn read_folder(root: &std::path::Path, dir: &str) -> Result<Vec<(String, String)>, String> {
    let path = root.join(dir);
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&path).map_err(|e| format!("{}: {e}", path.display()))? {
        let name = entry
            .map_err(|e| format!("{}: {e}", path.display()))?
            .file_name()
            .to_string_lossy()
            .into_owned();
        let text =
            std::fs::read_to_string(path.join(&name)).map_err(|e| format!("{dir}/{name}: {e}"))?;
        files.push((name, text));
    }
    files.sort();
    Ok(files)
}

/// Whether `s` is `<prefix>` followed by one or more digits.
fn is_prefixed_number(s: &str, prefix: &str) -> bool {
    s.strip_prefix(prefix)
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// Whether `s` names an NCBI Taxonomy id as VBCI's records write it: `NCBITaxon_<digits>`.
pub fn is_ncbi_taxon(s: &str) -> bool {
    is_prefixed_number(s, "NCBITaxon_")
}

/// Whether `s` is a Wikidata item: `Q<digits>`.
pub fn is_wikidata_item(s: &str) -> bool {
    is_prefixed_number(s, "Q")
}
