//! `diet/`: what decides each food's vegan and vegetarian answers. Four files, each with its own
//! schema: the class of each USDA category, the groups of the organisms Foundation foods record,
//! naming rules within a category, and exceptions for single foods.

use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// An answer for one diet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Answer {
    Yes,
    No,
    Unknown,
}

impl Answer {
    pub fn as_str(self) -> &'static str {
        match self {
            Answer::Yes => "yes",
            Answer::No => "no",
            Answer::Unknown => "unknown",
        }
    }
}

/// The class of a USDA category, which gives its foods' answers when nothing above it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Plant,
    DairyAndEgg,
    Flesh,
    Mixed,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Plant => "plant",
            Class::DairyAndEgg => "dairy_and_egg",
            Class::Flesh => "flesh",
            Class::Mixed => "mixed",
        }
    }
}

/// The group of an organism a Foundation food records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Organism {
    Animal,
    Plant,
    Fungus,
}

impl Organism {
    pub fn as_str(self) -> &'static str {
        match self {
            Organism::Animal => "animal",
            Organism::Plant => "plant",
            Organism::Fungus => "fungus",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DietCategory {
    /// USDA's description of the category, word for word.
    pub description: String,
    pub class: Class,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Taxon {
    /// The NCBI Taxonomy id, as `NCBITaxon_<digits>`.
    pub id: String,
    pub name: String,
    pub group: Organism,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// The category the rule applies within, as USDA describes it.
    pub category: String,
    /// The start of USDA's name for a food, exactly, case and all.
    pub starts_with: String,
    pub vegan: Option<Answer>,
    pub vegetarian: Option<Answer>,
    pub why: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exception {
    pub fdc_id: u32,
    /// USDA's name for the food, word for word.
    pub description: String,
    pub vegan: Option<Answer>,
    pub vegetarian: Option<Answer>,
    pub why: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CategoriesFile {
    category: Vec<DietCategory>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaxaFile {
    taxon: Vec<Taxon>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RulesFile {
    rule: Vec<Rule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExceptionsFile {
    food: Vec<Exception>,
}

/// The four records of `diet/`, parsed and checked, with the text each was parsed from.
#[derive(Debug)]
pub struct Diet {
    pub categories: Vec<DietCategory>,
    pub taxa: Vec<Taxon>,
    pub rules: Vec<Rule>,
    pub exceptions: Vec<Exception>,
    /// Each file's path, relative to the repo root, and its text.
    pub files: Vec<(String, String)>,
}

impl Diet {
    /// The folder, relative to the repo root.
    pub const DIR: &'static str = "diet";
    /// The files the folder holds, and no others.
    pub const FILES: [&'static str; 4] = [
        "categories.toml",
        "taxa.toml",
        "rules.toml",
        "exceptions.toml",
    ];

    /// Reads `diet/` under `root`, refusing a file the folder should not hold.
    pub fn load(root: &Path) -> Result<Diet, String> {
        let dir = root.join(Self::DIR);
        for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let name = entry
                .map_err(|e| format!("{}: {e}", dir.display()))?
                .file_name()
                .to_string_lossy()
                .into_owned();
            if !Self::FILES.contains(&name.as_str()) {
                return Err(format!(
                    "{}/{name}: not a diet record (the folder holds {})",
                    Self::DIR,
                    Self::FILES.join(", ")
                ));
            }
        }
        let mut texts = Vec::new();
        for file in Self::FILES {
            let path = dir.join(file);
            texts.push(fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?);
        }
        Self::parse(&texts[0], &texts[1], &texts[2], &texts[3])
    }

    /// Parses the four records' texts, in the order of `FILES`, and checks the rules their
    /// comments state.
    pub fn parse(
        categories: &str,
        taxa: &str,
        rules: &str,
        exceptions: &str,
    ) -> Result<Diet, String> {
        let path = |file: &str| format!("{}/{file}", Self::DIR);
        let at = |file: &str, e: String| format!("{}: {e}", path(file));
        let c: CategoriesFile =
            toml::from_str(categories).map_err(|e| at(Self::FILES[0], e.to_string()))?;
        let t: TaxaFile = toml::from_str(taxa).map_err(|e| at(Self::FILES[1], e.to_string()))?;
        let r: RulesFile = toml::from_str(rules).map_err(|e| at(Self::FILES[2], e.to_string()))?;
        let x: ExceptionsFile =
            toml::from_str(exceptions).map_err(|e| at(Self::FILES[3], e.to_string()))?;
        let diet = Diet {
            categories: c.category,
            taxa: t.taxon,
            rules: r.rule,
            exceptions: x.food,
            files: Self::FILES
                .iter()
                .zip([categories, taxa, rules, exceptions])
                .map(|(file, text)| (path(file), text.to_string()))
                .collect(),
        };
        diet.check().map_err(|(file, e)| at(file, e))?;
        Ok(diet)
    }

    fn check(&self) -> Result<(), (&'static str, String)> {
        let [categories, taxa, rules, exceptions] = Self::FILES;
        let mut seen = BTreeSet::new();
        for c in &self.categories {
            if c.description.trim().is_empty() {
                return Err((categories, "a category's description is empty".into()));
            }
            if !seen.insert(c.description.as_str()) {
                return Err((categories, format!("{:?} is listed twice", c.description)));
            }
        }
        let mut seen = BTreeSet::new();
        for t in &self.taxa {
            let id_ok =
                t.id.strip_prefix("NCBITaxon_")
                    .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
            if !id_ok {
                return Err((taxa, format!("{:?} is not NCBITaxon_<digits>", t.id)));
            }
            if t.name.trim().is_empty() {
                return Err((taxa, format!("{}: name is empty", t.id)));
            }
            if !seen.insert(t.id.as_str()) {
                return Err((taxa, format!("{} is listed twice", t.id)));
            }
        }
        let mut seen = BTreeSet::new();
        for r in &self.rules {
            let which = format!("{} / {:?}", r.category, r.starts_with);
            if r.category.trim().is_empty() || r.starts_with.is_empty() {
                return Err((
                    rules,
                    format!("{which}: a rule names a category and a start"),
                ));
            }
            if r.vegan.is_none() && r.vegetarian.is_none() {
                return Err((rules, format!("{which}: gives no answer")));
            }
            if r.why.trim().is_empty() {
                return Err((rules, format!("{which}: why is empty")));
            }
            if !seen.insert((r.category.as_str(), r.starts_with.as_str())) {
                return Err((rules, format!("{which}: listed twice")));
            }
        }
        let mut seen = BTreeSet::new();
        for x in &self.exceptions {
            if x.description.trim().is_empty() {
                return Err((exceptions, format!("{}: description is empty", x.fdc_id)));
            }
            if x.vegan.is_none() && x.vegetarian.is_none() {
                return Err((exceptions, format!("{}: gives no answer", x.fdc_id)));
            }
            if x.why.trim().is_empty() {
                return Err((exceptions, format!("{}: why is empty", x.fdc_id)));
            }
            if !seen.insert(x.fdc_id) {
                return Err((exceptions, format!("{} is listed twice", x.fdc_id)));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATEGORIES: &str = "[[category]]\ndescription = \"Vegetables\"\nclass = \"plant\"\n";
    const TAXA: &str =
        "[[taxon]]\nid = \"NCBITaxon_33208\"\nname = \"Metazoa\"\ngroup = \"animal\"\n";
    const RULES: &str = "[[rule]]\ncategory = \"Vegetables\"\nstarts_with = \"Potatoes, mashed\"\nvegan = \"unknown\"\nwhy = \"Milk.\"\n";
    const EXCEPTIONS: &str =
        "[[food]]\nfdc_id = 1\ndescription = \"A\"\nvegan = \"no\"\nwhy = \"Butter.\"\n";

    fn refused(texts: [&str; 4], file: &str, why: &str) {
        let err = Diet::parse(texts[0], texts[1], texts[2], texts[3]).expect_err("refused");
        assert!(
            err.starts_with(&format!("diet/{file}")),
            "{err:?} is not about {file}"
        );
        assert!(err.contains(why), "{err:?} does not say {why:?}");
    }

    fn with(text: &str, old: &str, new: &str) -> String {
        assert_eq!(text.matches(old).count(), 1, "{old:?}");
        text.replace(old, new)
    }

    #[test]
    fn good_diet_records_load() {
        let d = Diet::parse(CATEGORIES, TAXA, RULES, EXCEPTIONS).unwrap();
        assert_eq!(d.categories[0].class, Class::Plant);
        assert_eq!(d.taxa[0].group, Organism::Animal);
        assert_eq!(d.rules[0].vegan, Some(Answer::Unknown));
        assert_eq!(d.exceptions[0].vegetarian, None);
        assert_eq!(d.files[3].0, "diet/exceptions.toml");
    }

    #[test]
    fn a_diet_record_refuses_a_key_or_value_it_does_not_know() {
        let c = with(
            CATEGORIES,
            "class = \"plant\"",
            "class = \"plant\"\nnote = \"x\"",
        );
        refused(
            [&c, TAXA, RULES, EXCEPTIONS],
            "categories.toml",
            "unknown field `note`",
        );
        let c = with(CATEGORIES, "\"plant\"", "\"vegetable\"");
        refused(
            [&c, TAXA, RULES, EXCEPTIONS],
            "categories.toml",
            "unknown variant",
        );
        let t = with(TAXA, "\"animal\"", "\"bird\"");
        refused(
            [CATEGORIES, &t, RULES, EXCEPTIONS],
            "taxa.toml",
            "unknown variant",
        );
        let r = with(RULES, "\"unknown\"", "\"maybe\"");
        refused(
            [CATEGORIES, TAXA, &r, EXCEPTIONS],
            "rules.toml",
            "unknown variant",
        );
        let x = with(
            EXCEPTIONS,
            "vegan = \"no\"",
            "vegan = \"no\"\nreviewed = true",
        );
        refused(
            [CATEGORIES, TAXA, RULES, &x],
            "exceptions.toml",
            "unknown field `reviewed`",
        );
    }

    #[test]
    fn a_rule_or_exception_gives_an_answer_and_a_reason() {
        let r = with(RULES, "vegan = \"unknown\"\n", "");
        refused(
            [CATEGORIES, TAXA, &r, EXCEPTIONS],
            "rules.toml",
            "gives no answer",
        );
        let r = with(RULES, "why = \"Milk.\"", "why = \" \"");
        refused(
            [CATEGORIES, TAXA, &r, EXCEPTIONS],
            "rules.toml",
            "why is empty",
        );
        let x = with(EXCEPTIONS, "vegan = \"no\"\n", "");
        refused(
            [CATEGORIES, TAXA, RULES, &x],
            "exceptions.toml",
            "gives no answer",
        );
        let x = with(EXCEPTIONS, "why = \"Butter.\"", "why = \"\"");
        refused(
            [CATEGORIES, TAXA, RULES, &x],
            "exceptions.toml",
            "why is empty",
        );
    }

    #[test]
    fn diet_records_list_each_thing_once() {
        let c = format!("{CATEGORIES}{CATEGORIES}");
        refused(
            [&c, TAXA, RULES, EXCEPTIONS],
            "categories.toml",
            "listed twice",
        );
        let t = format!("{TAXA}{TAXA}");
        refused(
            [CATEGORIES, &t, RULES, EXCEPTIONS],
            "taxa.toml",
            "listed twice",
        );
        let r = format!("{RULES}{RULES}");
        refused(
            [CATEGORIES, TAXA, &r, EXCEPTIONS],
            "rules.toml",
            "listed twice",
        );
        let x = format!("{EXCEPTIONS}{EXCEPTIONS}");
        refused(
            [CATEGORIES, TAXA, RULES, &x],
            "exceptions.toml",
            "listed twice",
        );
    }

    #[test]
    fn a_taxon_is_named_by_its_ncbi_id() {
        for id in ["33208", "NCBITaxon_", "NCBITaxon_33a08", "ncbitaxon_33208"] {
            let t = with(TAXA, "NCBITaxon_33208", id);
            refused(
                [CATEGORIES, &t, RULES, EXCEPTIONS],
                "taxa.toml",
                "is not NCBITaxon_<digits>",
            );
        }
    }

    #[test]
    fn the_diet_folder_holds_its_four_records_and_no_other_file() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let diet = Diet::load(&root).unwrap();
        assert!(diet.categories.len() >= 28 && diet.exceptions.len() >= 60);
        let scratch =
            std::env::temp_dir().join(format!("balaena-records-diet-{}", std::process::id()));
        let _ = fs::remove_dir_all(&scratch);
        fs::create_dir_all(scratch.join(Diet::DIR)).unwrap();
        for file in Diet::FILES {
            fs::copy(
                root.join(Diet::DIR).join(file),
                scratch.join(Diet::DIR).join(file),
            )
            .unwrap();
        }
        assert!(Diet::load(&scratch).is_ok());
        fs::write(scratch.join(Diet::DIR).join("notes.md"), "x").unwrap();
        let err = Diet::load(&scratch).unwrap_err();
        fs::remove_dir_all(&scratch).unwrap();
        assert!(err.contains("diet/notes.md: not a diet record"), "{err}");
    }
}
