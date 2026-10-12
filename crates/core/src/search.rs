//! Ingredients and USDA foods found in the export by name, for writing an ingredient's aliases and
//! forms. No number is derived from what it finds.

use crate::Export;
use crate::recipe::{columns, integer, text};
use std::collections::BTreeMap;

/// A USDA food a search found.
#[derive(Debug, Clone, PartialEq)]
pub struct FoodFound {
    pub fdc_id: i64,
    pub dataset: String,
    /// USDA's name for it.
    pub description: String,
    pub vegan: String,
    pub vegetarian: String,
    /// The ingredient and state of each form that links it.
    pub forms: Vec<(String, String)>,
}

/// What a search found: each ingredient with the aliases that match, by id; and the foods whose
/// names match, the simplest first, those of fewest words, then by fdc_id.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub ingredients: Vec<(String, Vec<String>)>,
    pub foods: Vec<FoodFound>,
}

/// A name's words: its runs of letters and digits, lowercased.
pub fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Whether each of the query's words starts a word of `name`.
fn matches(query: &[String], name: &str) -> bool {
    let name = words(name);
    query.iter().all(|q| name.iter().any(|w| w.starts_with(q)))
}

/// The ingredients and foods whose names match `query`: each of its words starts a word of an
/// alias, or of USDA's name. Err on a query with no word.
pub fn search(export: &Export, query: &str) -> Result<Found, String> {
    let query = words(query);
    if query.is_empty() {
        return Err("a search needs a word of letters or digits".into());
    }
    let bad = |table: &str, n: usize| format!("the export's {table}: row {} lacks a value", n + 1);
    let (t, [alias, ingredient]) = columns(export, "ingredient_alias", ["alias", "ingredient"])?;
    let mut ingredients: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (n, r) in t.rows.iter().enumerate() {
        let (Some(a), Some(i)) = (text(&r[alias]), text(&r[ingredient])) else {
            return Err(bad("ingredient_alias", n));
        };
        if matches(&query, a) {
            ingredients
                .entry(i.to_string())
                .or_default()
                .push(a.to_string());
        }
    }
    let (t, [ingredient, state, fdc_id]) =
        columns(export, "ingredient_form", ["ingredient", "state", "fdc_id"])?;
    let mut forms: BTreeMap<i64, Vec<(String, String)>> = BTreeMap::new();
    for (n, r) in t.rows.iter().enumerate() {
        let (Some(i), Some(s), Some(f)) =
            (text(&r[ingredient]), text(&r[state]), integer(&r[fdc_id]))
        else {
            return Err(bad("ingredient_form", n));
        };
        forms
            .entry(f)
            .or_default()
            .push((i.to_string(), s.to_string()));
    }
    let (t, [fdc_id, dataset, description, vegan, vegetarian]) = columns(
        export,
        "food_diet",
        ["fdc_id", "dataset", "description", "vegan", "vegetarian"],
    )?;
    let mut foods = Vec::new();
    for (n, r) in t.rows.iter().enumerate() {
        let (Some(f), Some(ds), Some(d), Some(v), Some(g)) = (
            integer(&r[fdc_id]),
            text(&r[dataset]),
            text(&r[description]),
            text(&r[vegan]),
            text(&r[vegetarian]),
        ) else {
            return Err(bad("food_diet", n));
        };
        if matches(&query, d) {
            foods.push(FoodFound {
                fdc_id: f,
                dataset: ds.to_string(),
                description: d.to_string(),
                vegan: v.to_string(),
                vegetarian: g.to_string(),
                forms: forms.get(&f).cloned().unwrap_or_default(),
            });
        }
    }
    foods.sort_by_key(|f| (words(&f.description).len(), f.fdc_id));
    Ok(Found {
        ingredients: ingredients.into_iter().collect(),
        foods,
    })
}
