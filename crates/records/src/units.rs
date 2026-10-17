//! `units/count.toml`: count units, units a recipe or USDA's portion text counts in, which no mass
//! or volume unit converts. cooklang's bundled units give the mass and volume units.

use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountUnit {
    /// Lowercase letters; unique in the record.
    pub key: String,
    /// The names it may be written with, lowercase; each names this unit alone.
    pub names: Vec<String>,
    /// `true` where one of the unit is one whole item of a food, so a bare count, such as
    /// `@onion{1}`, whose name ends in no count unit's name, is weighed by a food's portions in it;
    /// left out where it is not.
    pub whole: Option<bool>,
}

impl CountUnit {
    /// Whether one of the unit is one whole item of a food.
    pub fn is_whole(&self) -> bool {
        self.whole == Some(true)
    }
}

/// The record, as `units/count.toml` holds it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountUnits {
    pub unit: Vec<CountUnit>,
}

impl CountUnits {
    /// The folder, relative to the repo root.
    pub const DIR: &'static str = "units";
    /// The record's path, relative to the repo root.
    pub const PATH: &'static str = "units/count.toml";

    /// Reads `units/count.toml` under `root`, refusing any other file in `units/`, and returns the
    /// record and its text.
    pub fn load(root: &Path) -> Result<(CountUnits, String), String> {
        let dir = root.join(Self::DIR);
        for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let name = entry
                .map_err(|e| format!("{}: {e}", dir.display()))?
                .file_name()
                .to_string_lossy()
                .into_owned();
            if name != "count.toml" {
                return Err(format!(
                    "{}/{name}: not a units record (the folder holds count.toml)",
                    Self::DIR
                ));
            }
        }
        let path = root.join(Self::PATH);
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let units = Self::parse(&text).map_err(|e| format!("{}: {e}", Self::PATH))?;
        Ok((units, text))
    }

    /// Parses the record and checks the rules its comments state that need no other record.
    pub fn parse(text: &str) -> Result<CountUnits, String> {
        let units: CountUnits = toml::from_str(text).map_err(|e| e.to_string())?;
        if units.unit.is_empty() {
            return Err("no unit".into());
        }
        let (mut keys, mut names) = (BTreeSet::new(), BTreeSet::new());
        for u in &units.unit {
            if u.key.is_empty() || !u.key.chars().all(|c| c.is_ascii_lowercase()) {
                return Err(format!("unit {:?}: a key is lowercase letters", u.key));
            }
            if !keys.insert(u.key.as_str()) {
                return Err(format!("unit {}: two units have this key", u.key));
            }
            if u.names.is_empty() {
                return Err(format!("unit {}: no name", u.key));
            }
            if u.whole == Some(false) {
                return Err(format!(
                    "unit {}: whole = false says nothing: leave whole out",
                    u.key
                ));
            }
            for name in &u.names {
                let plain = !name.is_empty()
                    && *name == name.to_lowercase()
                    && *name == name.split_whitespace().collect::<Vec<_>>().join(" ");
                if !plain {
                    return Err(format!(
                        "unit {}: name {name:?} is not lowercase with single spaces",
                        u.key
                    ));
                }
                if !names.insert(name.as_str()) {
                    return Err(format!(
                        "unit {}: name {name:?} names another unit too",
                        u.key
                    ));
                }
            }
        }
        Ok(units)
    }

    /// The unit `name` names, if any.
    pub fn find(&self, name: &str) -> Option<&CountUnit> {
        self.unit.iter().find(|u| u.names.iter().any(|n| n == name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "[[unit]]\nkey = \"clove\"\nnames = [\"clove\", \"cloves\"]\n\n[[unit]]\nkey = \"head\"\nnames = [\"head\"]\n";

    fn refused(text: &str, why: &str) {
        let err = CountUnits::parse(text).expect_err("refused");
        assert!(err.contains(why), "{err:?} does not say {why:?}");
    }

    #[test]
    fn a_count_unit_is_found_by_its_names() {
        let u = CountUnits::parse(GOOD).unwrap();
        assert_eq!(u.find("cloves").map(|c| c.key.as_str()), Some("clove"));
        assert!(u.find("Clove").is_none() && u.find("cup").is_none());
    }

    #[test]
    fn a_count_unit_record_refuses_what_it_does_not_know() {
        refused(
            &GOOD.replace("key = \"head\"", "key = \"head\"\ngrams = 3"),
            "unknown field `grams`",
        );
        refused(
            &GOOD.replace("\"cloves\"", "\"Cloves\""),
            "not lowercase with single spaces",
        );
        refused(
            &GOOD.replace("names = [\"head\"]", "names = [\"cloves\"]"),
            "names another unit too",
        );
        refused(
            &GOOD.replace("key = \"head\"", "key = \"clove\""),
            "two units have this key",
        );
        refused(
            &GOOD.replace("key = \"head\"", "key = \"Head\""),
            "lowercase letters",
        );
        refused(&GOOD.replace("names = [\"head\"]", "names = []"), "no name");
        refused(
            &GOOD.replace("names = [\"head\"]", "names = [\"head\"]\nwhole = false"),
            "unit head: whole = false says nothing",
        );
        refused(
            &GOOD.replace("names = [\"head\"]", "names = [\"head\"]\nwhole = \"yes\""),
            "invalid type",
        );
    }

    #[test]
    fn a_count_unit_is_whole_where_its_record_says_so() {
        let u = CountUnits::parse(
            &GOOD.replace("names = [\"head\"]", "names = [\"head\"]\nwhole = true"),
        )
        .unwrap();
        assert!(u.find("head").unwrap().is_whole());
        assert!(!u.find("clove").unwrap().is_whole());
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let (units, _) = CountUnits::load(&root).unwrap();
        let whole: Vec<&str> = units
            .unit
            .iter()
            .filter(|u| u.is_whole())
            .map(|u| u.key.as_str())
            .collect();
        assert_eq!(whole, ["head", "each", "small", "medium", "large", "fruit"]);
    }

    #[test]
    fn the_units_folder_holds_count_toml_alone() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let (units, _) = CountUnits::load(&root).unwrap();
        for name in ["clove", "bunch", "head", "can", "medium"] {
            assert!(units.find(name).is_some(), "{name}");
        }
        let scratch =
            std::env::temp_dir().join(format!("balaena-records-units-{}", std::process::id()));
        let _ = fs::remove_dir_all(&scratch);
        fs::create_dir_all(scratch.join(CountUnits::DIR)).unwrap();
        fs::copy(root.join(CountUnits::PATH), scratch.join(CountUnits::PATH)).unwrap();
        assert!(CountUnits::load(&scratch).is_ok());
        fs::write(scratch.join(CountUnits::DIR).join("mass.toml"), "x").unwrap();
        let err = CountUnits::load(&scratch).unwrap_err();
        fs::remove_dir_all(&scratch).unwrap();
        assert!(err.contains("units/mass.toml: not a units record"), "{err}");
    }
}
