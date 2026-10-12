//! `ingredients/<id>.toml`: VBCI's own ingredients, one file each. An ingredient has a name, the
//! aliases recipes may call it by, and one or more forms, each a state linked to one USDA food by
//! its fdc_id and USDA's name for it. The first form is the default, used when a recipe does not
//! say which state.
//!
//! An alias is another name for its ingredient's foods, not for a near one: a name USDA gives a food
//! of its own, such as Foundation's "Spinach, baby" or "Oil, olive, extra virgin", belongs to an
//! ingredient of its own. A review holds that, not a test.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
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
    form: Vec<Form>,
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

/// Whether `s` is lowercase letters and digits joined by single hyphens.
fn is_id(s: &str) -> bool {
    !s.is_empty()
        && s.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

impl Ingredients {
    /// The folder, relative to the repo root.
    pub const DIR: &'static str = "ingredients";

    /// Reads every file in `ingredients/` under `root`, refusing one that is not `<id>.toml`.
    pub fn load(root: &Path) -> Result<Ingredients, String> {
        let dir = root.join(Self::DIR);
        let mut files = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let name = entry
                .map_err(|e| format!("{}: {e}", dir.display()))?
                .file_name()
                .to_string_lossy()
                .into_owned();
            let text = fs::read_to_string(dir.join(&name))
                .map_err(|e| format!("{}/{name}: {e}", Self::DIR))?;
            files.push((name, text));
        }
        files.sort();
        Self::parse(&files)
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
                .filter(|id| is_id(id))
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
            if let Some(q) = &f.wikidata {
                let ok = q
                    .strip_prefix('Q')
                    .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
                if !ok {
                    return Err(at(format!("wikidata {q:?} is not Q<digits>")));
                }
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
            ingredient.push(Ingredient {
                id: id.to_string(),
                name: f.name,
                wikidata: f.wikidata,
                aliases: f.aliases,
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
