//! VBCI's records, typed. Each schema refuses a key it doesn't know, and each loader checks the
//! rules its record's comments state. Parsing a record needs no database and no network.

pub mod diet;
pub mod ingredients;
pub mod nutrients;
pub mod sources;
pub mod units;

pub use diet::{Answer, Class, Diet, DietCategory, Exception, Organism, Rule, Taxon};
pub use ingredients::{Form, Ingredient, IngredientOrganism, Ingredients, State};
pub use nutrients::{Nutrient, Nutrients};
pub use sources::{Dataset, Publisher, Sources, WikidataItem};
pub use units::{CountUnit, CountUnits};

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
