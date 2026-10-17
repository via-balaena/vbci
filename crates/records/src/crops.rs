//! `crops/<id>.toml`: the crops VBCI knows, one file each. A crop names the ingredient it yields and
//! the state the ingredient is in when harvested, and its organism: a Wikidata item `sources.toml`
//! pins, with the NCBI Taxonomy id and the scientific name that item gives. A crop names its
//! ingredient rather than being found through its organism, as one organism can be several crops.
//!
//! Which ingredient a crop yields is a review's choice. That the item gives the crop's taxon and
//! name, and that the crop's organism is its ingredient's or one step below it, by the item's
//! parent taxon, the warehouse's load checks (decided 2026-10-16, Jon: "your call").

use crate::State;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CropFile {
    name: String,
    wikidata: String,
    taxon: String,
    scientific_name: String,
    ingredient: String,
    state: State,
}

/// One crop, as its file holds it, with the id its file name gives it.
#[derive(Debug)]
pub struct Crop {
    /// The file's name without `.toml`, as an ingredient's is.
    pub id: String,
    pub name: String,
    /// The Wikidata item of the crop's organism, `Q<digits>`.
    pub wikidata: String,
    /// The NCBI Taxonomy id that item gives, as `NCBITaxon_<digits>`.
    pub taxon: String,
    /// The scientific name that item gives, word for word.
    pub scientific_name: String,
    /// The id of the ingredient the crop yields.
    pub ingredient: String,
    /// The state the ingredient is in when harvested.
    pub state: State,
    /// The file's path, relative to the repo root, and its text.
    pub path: String,
    pub text: String,
}

/// Every crop in `crops/`, by id.
#[derive(Debug)]
pub struct Crops {
    pub crop: Vec<Crop>,
}

impl Crops {
    /// The folder, relative to the repo root.
    pub const DIR: &'static str = "crops";

    /// Reads every file in `crops/` under `root`, refusing one that is not `<id>.toml`.
    pub fn load(root: &Path) -> Result<Crops, String> {
        Self::parse(&crate::read_folder(root, Self::DIR)?)
    }

    /// Parses each `(file name, text)` and checks the rules the module's docs state.
    pub fn parse(files: &[(String, String)]) -> Result<Crops, String> {
        let mut crop = Vec::new();
        for (file, text) in files {
            let path = format!("{}/{file}", Self::DIR);
            let id = file
                .strip_suffix(".toml")
                .filter(|id| crate::is_id(id))
                .ok_or_else(|| {
                    format!(
                        "{path}: a crop's file is named <id>.toml, its id lowercase letters and digits joined by hyphens"
                    )
                })?;
            let f: CropFile = toml::from_str(text).map_err(|e| format!("{path}: {e}"))?;
            let at = |e: String| format!("{path}: {e}");
            if f.name.trim().is_empty() {
                return Err(at("name is empty".into()));
            }
            if !crate::is_wikidata_item(&f.wikidata) {
                return Err(at(format!("wikidata {:?} is not Q<digits>", f.wikidata)));
            }
            if !crate::is_ncbi_taxon(&f.taxon) {
                return Err(at(format!("taxon {:?} is not NCBITaxon_<digits>", f.taxon)));
            }
            if f.scientific_name.trim().is_empty() {
                return Err(at("scientific_name is empty".into()));
            }
            if !crate::is_id(&f.ingredient) {
                return Err(at(format!(
                    "ingredient {:?} is not an ingredient's id",
                    f.ingredient
                )));
            }
            crop.push(Crop {
                id: id.to_string(),
                name: f.name,
                wikidata: f.wikidata,
                taxon: f.taxon,
                scientific_name: f.scientific_name,
                ingredient: f.ingredient,
                state: f.state,
                path,
                text: text.clone(),
            });
        }
        Ok(Crops { crop })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARROT: &str = r#"name = "Carrot"
wikidata = "Q11678009"
taxon = "NCBITaxon_79200"
scientific_name = "Daucus carota subsp. sativus"
ingredient = "carrot"
state = "raw"
"#;

    fn files(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(f, t)| (f.to_string(), t.to_string()))
            .collect()
    }

    fn refused(pairs: &[(&str, &str)], why: &str) {
        let err = Crops::parse(&files(pairs)).expect_err("refused");
        assert!(err.contains(why), "{err:?} does not say {why:?}");
    }

    /// `CARROT` with one replacement, which must occur exactly once.
    fn with(old: &str, new: &str) -> String {
        assert_eq!(CARROT.matches(old).count(), 1, "{old:?}");
        CARROT.replace(old, new)
    }

    #[test]
    fn a_good_crop_loads() {
        let c = Crops::parse(&files(&[("carrot.toml", CARROT)])).unwrap();
        let carrot = &c.crop[0];
        assert_eq!(
            (carrot.id.as_str(), carrot.path.as_str()),
            ("carrot", "crops/carrot.toml")
        );
        assert_eq!(
            (
                carrot.taxon.as_str(),
                carrot.ingredient.as_str(),
                carrot.state
            ),
            ("NCBITaxon_79200", "carrot", State::Raw)
        );
    }

    #[test]
    fn a_crop_file_refuses_a_key_or_value_it_does_not_know() {
        refused(
            &[(
                "carrot.toml",
                &with("name = \"Carrot\"", "name = \"Carrot\"\ngbif = 1"),
            )],
            "unknown field `gbif`",
        );
        refused(
            &[("carrot.toml", &with("state = \"raw\"", "state = \"fresh\""))],
            "unknown variant",
        );
        refused(
            &[("carrot.toml", &with("state = \"raw\"\n", ""))],
            "missing field `state`",
        );
    }

    #[test]
    fn a_crop_is_named_by_its_file() {
        for file in [
            "Carrot.toml",
            "carrot",
            "purple_carrot.toml",
            "-carrot.toml",
        ] {
            refused(&[(file, CARROT)], "named <id>.toml");
        }
        assert!(Crops::parse(&files(&[("purple-carrot-2.toml", CARROT)])).is_ok());
    }

    /// A crop's organism is a Wikidata item, `Q<digits>`, with a taxon written as `diet/taxa.toml`
    /// writes one and a scientific name; the ingredient is named by its id.
    #[test]
    fn a_crop_names_its_organism_and_its_ingredient_by_their_ids() {
        for (old, new, why) in [
            (
                "\"Q11678009\"",
                "\"q11678009\"",
                "wikidata \"q11678009\" is not Q<digits>",
            ),
            ("\"Q11678009\"", "\"Q\"", "wikidata \"Q\" is not Q<digits>"),
            (
                "\"NCBITaxon_79200\"",
                "\"79200\"",
                "taxon \"79200\" is not NCBITaxon_<digits>",
            ),
            (
                "\"Daucus carota subsp. sativus\"",
                "\" \"",
                "scientific_name is empty",
            ),
            (
                "ingredient = \"carrot\"",
                "ingredient = \"Carrot\"",
                "ingredient \"Carrot\" is not an ingredient's id",
            ),
            ("name = \"Carrot\"", "name = \"\"", "name is empty"),
        ] {
            refused(&[("carrot.toml", &with(old, new))], why);
        }
    }

    #[test]
    fn the_crops_on_disk_load() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let c = Crops::load(&root).unwrap();
        assert!(c.crop.len() >= 10);
        for id in ["carrot", "cherry-tomato", "tomato", "cumin"] {
            assert!(c.crop.iter().any(|g| g.id == id), "{id}");
        }
    }
}
