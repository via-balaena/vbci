//! Reading the unit of a USDA portion from its text, with cooklang's bundled mass and volume units
//! and the count units `units/count.toml` lists.

use balaena_records::CountUnits;
use cooklang::convert::{Converter, PhysicalQuantity};

/// What a portion's unit is.
#[derive(Debug, Clone, PartialEq)]
pub struct PortionUnit {
    /// The words of the text that named the unit.
    pub phrase: String,
    /// `mass`, `volume` or `count`.
    pub kind: &'static str,
    /// cooklang's symbol for a mass or volume unit (`c` for cup), or the count unit's key.
    pub unit: String,
    /// Grams in one mass unit, or millilitres in one volume unit; none for a count unit.
    pub per_unit: Option<f64>,
}

/// The text a portion's unit is read from, and the leading phrase of it: lowercased, up to its
/// first comma or parenthesis.
pub fn leading_phrase(text: &str) -> String {
    let lower = text.to_lowercase();
    let end = lower.find([',', '(']).unwrap_or(lower.len());
    lower[..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The unit a portion's text names: the longest run of the leading phrase's first words that is
/// a mass or volume unit cooklang knows, or a count unit. None when no run is: the portion is
/// listed as unread, not guessed.
pub fn portion_unit(text: &str, converter: &Converter, counts: &CountUnits) -> Option<PortionUnit> {
    let phrase = leading_phrase(text);
    let words: Vec<&str> = phrase.split(' ').filter(|w| !w.is_empty()).collect();
    for n in (1..=words.len()).rev() {
        let candidate = words[..n].join(" ");
        if let Some(unit) = converter.find_unit(&candidate) {
            let per_unit = match unit.physical_quantity {
                PhysicalQuantity::Mass => unit.ratio,
                PhysicalQuantity::Volume => unit.ratio * 1000.0,
                _ => continue,
            };
            let kind = match unit.physical_quantity {
                PhysicalQuantity::Mass => "mass",
                _ => "volume",
            };
            let symbol = unit
                .symbols
                .first()
                .or(unit.names.first())
                .map(|s| s.to_string());
            return Some(PortionUnit {
                phrase: candidate,
                kind,
                unit: symbol.unwrap_or_default(),
                per_unit: Some(per_unit),
            });
        }
        if let Some(count) = counts.find(&candidate) {
            return Some(PortionUnit {
                phrase: candidate,
                kind: "count",
                unit: count.key.clone(),
                per_unit: None,
            });
        }
    }
    None
}

/// Refuses a count unit's name that cooklang already knows as a unit: a recipe's "c" or "cup"
/// must mean one thing.
pub fn check_count_units(counts: &CountUnits, converter: &Converter) -> Result<(), String> {
    for u in &counts.unit {
        for name in &u.names {
            if converter.find_unit(name).is_some() {
                return Err(format!(
                    "{}: count unit {}'s name {name:?} is a unit cooklang knows",
                    CountUnits::PATH,
                    u.key
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts() -> CountUnits {
        CountUnits::parse(
            "[[unit]]\nkey = \"medium\"\nnames = [\"medium\"]\n\n[[unit]]\nkey = \"can\"\nnames = [\"can\", \"cans\"]\n\n[[unit]]\nkey = \"each\"\nnames = [\"each\"]\n",
        )
        .unwrap()
    }

    fn read(text: &str) -> Option<(String, &'static str, String, Option<f64>)> {
        portion_unit(text, &Converter::bundled(), &counts()).map(|u| {
            (
                u.phrase,
                u.kind,
                u.unit,
                u.per_unit.map(|x| (x * 1000.0).round() / 1000.0),
            )
        })
    }

    #[test]
    fn what_a_portion_s_unit_is_read_as() {
        let s = |x: &str| x.to_string();
        assert_eq!(
            read("fl oz"),
            Some((s("fl oz"), "volume", s("fl oz"), Some(29.574)))
        );
        assert_eq!(
            read("Tbsp"),
            Some((s("tbsp"), "volume", s("tbsp"), Some(14.787)))
        );
        assert_eq!(
            read("cup, chopped"),
            Some((s("cup"), "volume", s("c"), Some(236.588)))
        );
        assert_eq!(
            read("cup whole"),
            Some((s("cup"), "volume", s("c"), Some(236.588)))
        );
        assert_eq!(
            read("tsp, rounded"),
            Some((s("tsp"), "volume", s("tsp"), Some(4.929)))
        );
        assert_eq!(
            read("milliliter"),
            Some((s("milliliter"), "volume", s("ml"), Some(1.0)))
        );
        assert_eq!(read("oz"), Some((s("oz"), "mass", s("oz"), Some(28.35))));
        assert_eq!(read("lb"), Some((s("lb"), "mass", s("lb"), Some(453.592))));
        assert_eq!(
            read("medium (2-1/4\" dia)"),
            Some((s("medium"), "count", s("medium"), None))
        );
        assert_eq!(
            read("can (303 x 406)"),
            Some((s("can"), "count", s("can"), None))
        );
        assert_eq!(read("each"), Some((s("each"), "count", s("each"), None)));
    }

    /// Text whose leading words name no mass, volume or count unit is not read: a length is not a
    /// mass or volume, and a unit later in the text is not the portion's unit.
    #[test]
    fn what_a_portion_s_unit_is_not_read_as() {
        for text in ["serving 8 oz", "inch", "cubic inch", "steak", "", "(1 cup)"] {
            assert_eq!(read(text), None, "{text:?}");
        }
    }

    /// A shorter run of words never wins over a longer one: with a count unit named "fl", "fl oz"
    /// is still a fluid ounce, and "fl" alone is the count unit.
    #[test]
    fn the_longest_run_that_names_a_unit_wins() {
        let fl = CountUnits::parse("[[unit]]\nkey = \"fl\"\nnames = [\"fl\"]\n").unwrap();
        let read =
            |text: &str| portion_unit(text, &Converter::bundled(), &fl).map(|u| (u.kind, u.unit));
        assert_eq!(read("fl oz"), Some(("volume", "fl oz".to_string())));
        assert_eq!(read("fl"), Some(("count", "fl".to_string())));
    }

    #[test]
    fn a_count_unit_may_not_be_a_unit_cooklang_knows() {
        let converter = Converter::bundled();
        assert!(check_count_units(&counts(), &converter).is_ok());
        let clash = CountUnits::parse("[[unit]]\nkey = \"cup\"\nnames = [\"cups\"]\n").unwrap();
        let err = check_count_units(&clash, &converter).unwrap_err();
        assert!(
            err.contains("count unit cup's name \"cups\" is a unit cooklang knows"),
            "{err}"
        );
    }
}
