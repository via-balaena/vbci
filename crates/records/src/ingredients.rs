//! `ingredients/<id>.toml`: VBCI's own ingredients, one file each. An ingredient has a name, the
//! aliases recipes may call it by, and one or more forms, each a state linked to one USDA food by
//! its fdc_id and USDA's name for it. The first form is the default, used when a recipe does not
//! say which state.
//!
//! An alias is another name for its ingredient's foods, not for a near one: a name USDA gives a food
//! of its own, such as Foundation's "Spinach, baby" or "Oil, olive, extra virgin", belongs to an
//! ingredient of its own. A review holds that, not a test.
//!
//! Every ingredient says, in its `[organism]` table, the organism it comes from by its NCBI
//! Taxonomy id and where that id is read from, or why it has none (decided with Jon, 2026-10-16):
//!
//! - a Foundation food, by `fdc_id` and USDA's name for it, whose "NCBI Taxon" USDA's data gives;
//! - else a Wikidata item `sources.toml` pins, by `wikidata`: the item's NCBI Taxonomy id, or that
//!   of the item its "natural product of taxon" names;
//! - or `none`, saying why: a mix of organisms, such as a broth, has none.
//!
//! Which food or item is the ingredient's organism's is a review's choice; that USDA's data or the
//! pinned item gives the taxon the file says, the warehouse's load checks.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The state an ingredient is in, as a recipe uses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Raw,
    Dry,
    Cooked,
    Canned,
    Frozen,
    /// A product used as it is bought, such as an oil, a broth or a cheese.
    AsSold,
}

impl State {
    /// Every state, in the order the enum lists them.
    pub const ALL: [State; 6] = [
        State::Raw,
        State::Dry,
        State::Cooked,
        State::Canned,
        State::Frozen,
        State::AsSold,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            State::Raw => "raw",
            State::Dry => "dry",
            State::Cooked => "cooked",
            State::Canned => "canned",
            State::Frozen => "frozen",
            State::AsSold => "as_sold",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Form {
    pub state: State,
    pub fdc_id: u32,
    /// USDA's name for the food, word for word.
    pub description: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IngredientFile {
    name: String,
    wikidata: Option<String>,
    aliases: Vec<String>,
    organism: OrganismFile,
    form: Vec<Form>,
}

/// An ingredient's `[organism]` table, as written: which keys go together is checked in `parse`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OrganismFile {
    taxon: Option<String>,
    fdc_id: Option<u32>,
    description: Option<String>,
    wikidata: Option<String>,
    none: Option<String>,
}

/// The organism an ingredient comes from, by its NCBI Taxonomy id as `NCBITaxon_<digits>`, and
/// where that id is read from; or why the ingredient has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngredientOrganism {
    /// Read from the "NCBI Taxon" USDA's data gives a Foundation food, named by its fdc_id and
    /// USDA's name for it, word for word.
    Usda {
        taxon: String,
        fdc_id: u32,
        description: String,
    },
    /// Read from a Wikidata item, `Q<digits>`, which `sources.toml` pins.
    Wikidata { taxon: String, item: String },
    /// None, and why.
    None { why: String },
}

impl IngredientOrganism {
    /// The NCBI Taxonomy id, or None for an ingredient with no organism.
    pub fn taxon(&self) -> Option<&str> {
        match self {
            IngredientOrganism::Usda { taxon, .. } | IngredientOrganism::Wikidata { taxon, .. } => {
                Some(taxon)
            }
            IngredientOrganism::None { .. } => None,
        }
    }
}

impl OrganismFile {
    /// The table as an organism, refusing keys that do not go together: one place it is read
    /// from, a taxon with it unless it is none, and a description with an fdc_id alone.
    fn read(self) -> Result<IngredientOrganism, String> {
        let taxon = |t: Option<String>| -> Result<String, String> {
            let t = t.ok_or("organism: no taxon")?;
            if !crate::is_ncbi_taxon(&t) {
                return Err(format!("organism: taxon {t:?} is not NCBITaxon_<digits>"));
            }
            Ok(t)
        };
        if self.description.is_some() && self.fdc_id.is_none() {
            return Err("organism: a description goes with an fdc_id".into());
        }
        match (self.fdc_id, self.wikidata, self.none) {
            (Some(fdc_id), None, None) => {
                let description = self
                    .description
                    .filter(|d| !d.trim().is_empty())
                    .ok_or_else(|| format!("organism: food {fdc_id} has no description"))?;
                Ok(IngredientOrganism::Usda {
                    taxon: taxon(self.taxon)?,
                    fdc_id,
                    description,
                })
            }
            (None, Some(item), None) => {
                if !crate::is_wikidata_item(&item) {
                    return Err(format!("organism: wikidata {item:?} is not Q<digits>"));
                }
                Ok(IngredientOrganism::Wikidata {
                    taxon: taxon(self.taxon)?,
                    item,
                })
            }
            (None, None, Some(why)) => {
                if let Some(t) = self.taxon {
                    return Err(format!("organism: none, yet taxon {t:?}"));
                }
                if why.trim().is_empty() {
                    return Err("organism: none, with no reason".into());
                }
                Ok(IngredientOrganism::None { why })
            }
            _ => Err(
                "organism: read from one place: a Foundation food (fdc_id), a Wikidata item \
                 (wikidata), or none (none, saying why)"
                    .into(),
            ),
        }
    }
}

/// One ingredient, as its file holds it, with the id its file name gives it.
#[derive(Debug)]
pub struct Ingredient {
    /// The file's name without `.toml`: lowercase letters and digits, joined by single hyphens.
    pub id: String,
    pub name: String,
    /// A Wikidata item, as `Q<digits>`.
    pub wikidata: Option<String>,
    /// Lowercase, with single spaces; each names this ingredient alone.
    pub aliases: Vec<String>,
    pub organism: IngredientOrganism,
    /// First form first: the default.
    pub forms: Vec<Form>,
    /// The file's path, relative to the repo root, and its text.
    pub path: String,
    pub text: String,
}

/// Every ingredient in `ingredients/`, by id.
#[derive(Debug)]
pub struct Ingredients {
    pub ingredient: Vec<Ingredient>,
}

impl Ingredients {
    /// The folder, relative to the repo root.
    pub const DIR: &'static str = "ingredients";

    /// Reads every file in `ingredients/` under `root`, refusing one that is not `<id>.toml`.
    pub fn load(root: &Path) -> Result<Ingredients, String> {
        Self::parse(&crate::read_folder(root, Self::DIR)?)
    }

    /// Parses each `(file name, text)` and checks the rules the module's docs state, within each
    /// file and across them.
    pub fn parse(files: &[(String, String)]) -> Result<Ingredients, String> {
        let mut ingredient = Vec::new();
        let mut claimed: BTreeMap<String, String> = BTreeMap::new();
        for (file, text) in files {
            let path = format!("{}/{file}", Self::DIR);
            let id = file
                .strip_suffix(".toml")
                .filter(|id| crate::is_id(id))
                .ok_or_else(|| {
                    format!(
                        "{path}: an ingredient's file is named <id>.toml, its id lowercase letters and digits joined by hyphens"
                    )
                })?;
            let f: IngredientFile = toml::from_str(text).map_err(|e| format!("{path}: {e}"))?;
            let at = |e: String| format!("{path}: {e}");
            if f.name.trim().is_empty() {
                return Err(at("name is empty".into()));
            }
            if let Some(q) = &f.wikidata
                && !crate::is_wikidata_item(q)
            {
                return Err(at(format!("wikidata {q:?} is not Q<digits>")));
            }
            if f.aliases.is_empty() {
                return Err(at("no alias: a recipe could not name it".into()));
            }
            let mut own = BTreeSet::new();
            for alias in &f.aliases {
                let plain = !alias.is_empty()
                    && *alias == alias.to_lowercase()
                    && *alias == alias.split_whitespace().collect::<Vec<_>>().join(" ");
                if !plain {
                    return Err(at(format!(
                        "alias {alias:?} is not lowercase with single spaces"
                    )));
                }
                if !own.insert(alias.as_str()) {
                    return Err(at(format!("alias {alias:?} is listed twice")));
                }
                if let Some(other) = claimed.insert(alias.clone(), id.to_string()) {
                    return Err(at(format!(
                        "alias {alias:?} is claimed by ingredient {other} too"
                    )));
                }
            }
            if f.form.is_empty() {
                return Err(at("no form".into()));
            }
            let mut states = BTreeSet::new();
            for form in &f.form {
                if !states.insert(form.state.as_str()) {
                    return Err(at(format!(
                        "two forms are {}: one food per state",
                        form.state.as_str()
                    )));
                }
                if form.description.trim().is_empty() {
                    return Err(at(format!("{}: description is empty", form.fdc_id)));
                }
            }
            let organism = f.organism.read().map_err(at)?;
            ingredient.push(Ingredient {
                id: id.to_string(),
                name: f.name,
                wikidata: f.wikidata,
                aliases: f.aliases,
                organism,
                forms: f.form,
                path,
                text: text.clone(),
            });
        }
        Ok(Ingredients { ingredient })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONION: &str = r#"name = "Onion"
aliases = ["onion", "onions"]

[organism]
taxon = "NCBITaxon_4679"
fdc_id = 790646
description = "Onions, yellow, raw"

[[form]]
state = "raw"
fdc_id = 170000
description = "Onions, raw"

[[form]]
state = "cooked"
fdc_id = 170001
description = "Onions, cooked, boiled, drained, without salt"
"#;

    fn files(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(f, t)| (f.to_string(), t.to_string()))
            .collect()
    }

    fn refused(pairs: &[(&str, &str)], why: &str) {
        let err = Ingredients::parse(&files(pairs)).expect_err("refused");
        assert!(err.contains(why), "{err:?} does not say {why:?}");
    }

    fn with(old: &str, new: &str) -> String {
        assert_eq!(ONION.matches(old).count(), 1, "{old:?}");
        ONION.replace(old, new)
    }

    #[test]
    fn a_good_ingredient_loads_with_its_first_form_as_default() {
        let i = Ingredients::parse(&files(&[("onion.toml", ONION)])).unwrap();
        let onion = &i.ingredient[0];
        assert_eq!(
            (onion.id.as_str(), onion.path.as_str()),
            ("onion", "ingredients/onion.toml")
        );
        assert_eq!(onion.forms[0].state, State::Raw);
        assert_eq!(onion.aliases, ["onion", "onions"]);
    }

    #[test]
    fn an_ingredient_file_refuses_a_key_or_value_it_does_not_know() {
        refused(
            &[(
                "onion.toml",
                &with("name = \"Onion\"", "name = \"Onion\"\nnote = \"x\""),
            )],
            "unknown field `note`",
        );
        refused(
            &[("onion.toml", &with("state = \"raw\"", "state = \"fresh\""))],
            "unknown variant",
        );
        refused(
            &[(
                "onion.toml",
                &with("fdc_id = 170000", "fdc_id = 170000\ngrams = 1"),
            )],
            "unknown field `grams`",
        );
    }

    #[test]
    fn an_ingredient_is_named_by_its_file() {
        for file in [
            "Onion.toml",
            "onion",
            "red_onion.toml",
            "red--onion.toml",
            "-onion.toml",
            "notes.md",
        ] {
            refused(&[(file, ONION)], "named <id>.toml");
        }
        assert!(Ingredients::parse(&files(&[("red-onion-2.toml", ONION)])).is_ok());
    }

    #[test]
    fn an_alias_is_plain_and_names_one_ingredient() {
        refused(
            &[("onion.toml", &with("\"onions\"", "\"Onions\""))],
            "not lowercase with single spaces",
        );
        refused(
            &[("onion.toml", &with("\"onions\"", "\"red  onion\""))],
            "not lowercase with single spaces",
        );
        refused(
            &[("onion.toml", &with("\"onions\"", "\" onions\""))],
            "not lowercase with single spaces",
        );
        refused(
            &[("onion.toml", &with("\"onions\"", "\"onion\""))],
            "listed twice",
        );
        refused(
            &[(
                "onion.toml",
                &with("aliases = [\"onion\", \"onions\"]", "aliases = []"),
            )],
            "no alias",
        );
        let shallot = ONION.replace(
            "aliases = [\"onion\", \"onions\"]",
            "aliases = [\"shallot\", \"onions\"]",
        );
        refused(
            &[("onion.toml", ONION), ("shallot.toml", &shallot)],
            "alias \"onions\" is claimed by ingredient onion too",
        );
    }

    #[test]
    fn an_ingredient_has_one_food_per_state() {
        refused(
            &[("onion.toml", &with("state = \"cooked\"", "state = \"raw\""))],
            "two forms are raw",
        );
        let no_form = &ONION[..ONION.find("[[form]]").unwrap()];
        refused(&[("onion.toml", no_form)], "missing field `form`");
        refused(
            &[(
                "onion.toml",
                &with("description = \"Onions, raw\"", "description = \"\""),
            )],
            "description is empty",
        );
    }

    #[test]
    fn a_wikidata_item_is_q_and_digits() {
        for q in ["\"q42\"", "\"Q\"", "\"Q4x\"", "\"42\""] {
            refused(
                &[(
                    "onion.toml",
                    &with(
                        "name = \"Onion\"",
                        &format!("name = \"Onion\"\nwikidata = {q}"),
                    ),
                )],
                "is not Q<digits>",
            );
        }
        assert!(
            Ingredients::parse(&files(&[(
                "onion.toml",
                &with(
                    "name = \"Onion\"",
                    "name = \"Onion\"\nwikidata = \"Q23485\""
                )
            )]))
            .is_ok()
        );
    }

    /// `ONION`'s `[organism]` table.
    const ONION_ORGANISM: &str = "[organism]\ntaxon = \"NCBITaxon_4679\"\nfdc_id = 790646\ndescription = \"Onions, yellow, raw\"\n";

    /// `ONION` with its `[organism]` table replaced by `table`.
    fn with_organism(table: &str) -> String {
        with(ONION_ORGANISM, table)
    }

    fn organism(text: &str) -> Result<IngredientOrganism, String> {
        Ingredients::parse(&files(&[("onion.toml", text)]))
            .map(|i| i.ingredient.into_iter().next().unwrap().organism)
    }

    /// An organism is read from one place, a Foundation food with USDA's name for it, a Wikidata
    /// item, or none with its reason; each but none has a taxon, and a description goes with a food
    /// alone. A file with no `[organism]` table is refused.
    #[test]
    fn an_ingredient_names_its_organism_or_why_it_has_none() {
        let taxon = "taxon = \"NCBITaxon_4679\"\n";
        let food = "fdc_id = 790646\ndescription = \"Onions, yellow, raw\"\n";
        let item = "wikidata = \"Q23485\"\n";
        let none = "none = \"A mix.\"\n";
        assert_eq!(
            organism(ONION),
            Ok(IngredientOrganism::Usda {
                taxon: "NCBITaxon_4679".into(),
                fdc_id: 790646,
                description: "Onions, yellow, raw".into()
            })
        );
        assert_eq!(
            organism(&with_organism(&format!("[organism]\n{taxon}{item}"))),
            Ok(IngredientOrganism::Wikidata {
                taxon: "NCBITaxon_4679".into(),
                item: "Q23485".into()
            })
        );
        assert_eq!(
            organism(&with_organism(&format!("[organism]\n{none}"))),
            Ok(IngredientOrganism::None {
                why: "A mix.".into()
            })
        );
        for (table, why) in [
            (String::new(), "missing field `organism`"),
            (format!("[organism]\n{taxon}"), "read from one place"),
            (
                format!("[organism]\n{taxon}{food}{item}"),
                "read from one place",
            ),
            (
                format!("[organism]\n{taxon}{food}{none}"),
                "read from one place",
            ),
            (format!("[organism]\n{item}{none}"), "read from one place"),
            (
                format!("[organism]\n{taxon}fdc_id = 790646\n"),
                "organism: food 790646 has no description",
            ),
            (
                format!("[organism]\n{taxon}fdc_id = 790646\ndescription = \" \"\n"),
                "organism: food 790646 has no description",
            ),
            (
                format!("[organism]\n{taxon}{item}description = \"Onions\"\n"),
                "a description goes with an fdc_id",
            ),
            (
                format!("[organism]\n{none}description = \"Onions\"\n"),
                "a description goes with an fdc_id",
            ),
            (format!("[organism]\n{food}"), "organism: no taxon"),
            (format!("[organism]\n{item}"), "organism: no taxon"),
            (
                format!("[organism]\n{taxon}{none}"),
                "organism: none, yet taxon \"NCBITaxon_4679\"",
            ),
            (
                "[organism]\nnone = \" \"\n".into(),
                "organism: none, with no reason",
            ),
            (
                format!("[organism]\n{taxon}wikidata = \"q23485\"\n"),
                "organism: wikidata \"q23485\" is not Q<digits>",
            ),
            (
                format!("[organism]\n{taxon}{food}grade = \"trusted\"\n"),
                "unknown field `grade`",
            ),
        ] {
            refused(&[("onion.toml", &with_organism(&table))], why);
        }
    }

    /// An organism's taxon is written as `diet/taxa.toml` writes one, whichever place it is read
    /// from.
    #[test]
    fn an_organism_is_named_by_its_ncbi_id() {
        for bad in [
            "4679",
            "NCBITaxon_",
            "NCBITaxon_46x9",
            "ncbitaxon_4679",
            "NCBITaxon_ 4679",
        ] {
            for rest in [
                "fdc_id = 790646\ndescription = \"Onions, yellow, raw\"\n",
                "wikidata = \"Q23485\"\n",
            ] {
                refused(
                    &[(
                        "onion.toml",
                        &with_organism(&format!("[organism]\ntaxon = \"{bad}\"\n{rest}")),
                    )],
                    "is not NCBITaxon_<digits>",
                );
            }
        }
    }

    #[test]
    fn the_ingredients_on_disk_load() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let i = Ingredients::load(&root).unwrap();
        assert!(i.ingredient.len() >= 16);
        for id in [
            "red-lentils",
            "onion",
            "garlic",
            "olive-oil",
            "vegetable-broth",
            "cumin",
            "lemon-juice",
            "parmesan",
        ] {
            assert!(
                i.ingredient.iter().any(|g| g.id == id),
                "the red lentil soup needs {id}"
            );
        }
    }
}
