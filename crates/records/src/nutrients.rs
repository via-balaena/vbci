//! `nutrients.toml`: the nutrients VBCI reports, each found in USDA's data by its codes, exactly as
//! USDA writes them, in the order a food's value is taken from them.

use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// The record, as `nutrients.toml` holds it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Nutrients {
    pub nutrient: Vec<Nutrient>,
}

/// One nutrient VBCI reports, per 100 g of food.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Nutrient {
    /// Lowercase letters, digits and underscores; unique in the record.
    pub key: String,
    pub name: String,
    /// The unit, as USDA writes it (`KCAL`, `G`, `MG`, `UG`); every code must have it.
    pub unit: String,
    /// USDA's nutrient_nbr codes, as text, first choice first. A code is in no other nutrient.
    pub codes: Vec<String>,
}

impl Nutrients {
    /// The record's path, relative to the repo root.
    pub const PATH: &'static str = "nutrients.toml";

    /// Reads `nutrients.toml` under `root`, returning the record and the text it was parsed from.
    pub fn load(root: &Path) -> Result<(Nutrients, String), String> {
        let path = root.join(Self::PATH);
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let nutrients = Self::parse(&text).map_err(|e| format!("{}: {e}", Self::PATH))?;
        Ok((nutrients, text))
    }

    /// Parses the record and checks the rules its comments state.
    pub fn parse(text: &str) -> Result<Nutrients, String> {
        let nutrients: Nutrients = toml::from_str(text).map_err(|e| e.to_string())?;
        nutrients.check()?;
        Ok(nutrients)
    }

    fn check(&self) -> Result<(), String> {
        if self.nutrient.is_empty() {
            return Err("no nutrient".into());
        }
        let (mut keys, mut codes) = (BTreeSet::new(), BTreeSet::new());
        for n in &self.nutrient {
            let key = &n.key;
            let key_ok = !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            if !key_ok {
                return Err(format!(
                    "nutrient {key:?}: a key is lowercase letters, digits and underscores"
                ));
            }
            if !keys.insert(key.as_str()) {
                return Err(format!("nutrient {key}: two nutrients have this key"));
            }
            for (field, value) in [("name", &n.name), ("unit", &n.unit)] {
                if value.trim().is_empty() {
                    return Err(format!("nutrient {key}: {field} is empty"));
                }
            }
            if n.codes.is_empty() {
                return Err(format!("nutrient {key}: no code"));
            }
            for code in &n.codes {
                if !is_code(code) {
                    return Err(format!(
                        "nutrient {key}: code {code:?} is not digits, or digits, a point and digits"
                    ));
                }
                if !codes.insert(code.as_str()) {
                    return Err(format!("nutrient {key}: code {code} is listed twice"));
                }
            }
        }
        Ok(())
    }
}

/// Whether `s` is written as USDA writes a nutrient_nbr: digits, or digits, a point and digits.
fn is_code(s: &str) -> bool {
    let digits = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit());
    match s.split_once('.') {
        None => digits(s),
        Some((whole, part)) => digits(whole) && digits(part),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
[[nutrient]]
key = "energy"
name = "Energy"
unit = "KCAL"
codes = ["208", "958"]

[[nutrient]]
key = "sugars"
name = "Sugars"
unit = "G"
codes = ["269", "269.3"]
"#;

    /// `GOOD` with one replacement, which must occur exactly once.
    fn with(old: &str, new: &str) -> String {
        assert_eq!(GOOD.matches(old).count(), 1, "{old:?}");
        GOOD.replace(old, new)
    }

    fn refused(text: &str, why: &str) {
        let err = Nutrients::parse(text).expect_err("refused");
        assert!(err.contains(why), "{err:?} does not say {why:?}");
    }

    #[test]
    fn a_good_nutrient_record_loads() {
        let n = Nutrients::parse(GOOD).unwrap();
        assert_eq!(n.nutrient[1].codes, ["269", "269.3"]);
    }

    #[test]
    fn a_nutrient_record_refuses_a_key_it_does_not_know() {
        refused(&format!("extra = 1\n{GOOD}"), "unknown field `extra`");
        refused(
            &with("unit = \"KCAL\"", "unit = \"KCAL\"\ndv = 2000"),
            "unknown field `dv`",
        );
    }

    #[test]
    fn a_code_is_written_as_usda_writes_it() {
        refused(&with("\"269.3\"", "269.3"), "invalid type");
        refused(&with("\"269.3\"", "\"269.\""), "not digits");
        refused(&with("\"269.3\"", "\".3\""), "not digits");
        refused(&with("\"269.3\"", "\"269,3\""), "not digits");
        refused(&with("\"958\"", "\" 958\""), "not digits");
        assert!(is_code("269.3") && is_code("208") && !is_code("") && !is_code("2.6.9"));
    }

    #[test]
    fn a_code_belongs_to_one_nutrient() {
        refused(
            &with("\"269\", \"269.3\"", "\"269\", \"208\""),
            "listed twice",
        );
        refused(
            &with("\"208\", \"958\"", "\"208\", \"208\""),
            "listed twice",
        );
    }

    #[test]
    fn nutrient_keys_are_unique_and_plain() {
        refused(
            &with("key = \"sugars\"", "key = \"energy\""),
            "two nutrients have this key",
        );
        refused(
            &with("key = \"sugars\"", "key = \"Sugars\""),
            "lowercase letters",
        );
        refused(&with("codes = [\"208\", \"958\"]", "codes = []"), "no code");
        refused(&with("unit = \"G\"", "unit = \"\""), "unit is empty");
    }

    #[test]
    fn the_nutrients_record_loads() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let (nutrients, _) = Nutrients::load(&root).unwrap();
        assert!(nutrients.nutrient.len() >= 16);
        let sugars = nutrients
            .nutrient
            .iter()
            .find(|n| n.key == "sugars")
            .unwrap();
        assert_eq!(sugars.codes, ["269", "269.3"]);
    }
}
