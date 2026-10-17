//! A recipe analysed against the export: each ingredient line's food, grams and diet answers, and
//! the recipe's nutrient totals and diet answers. cooklang parses the recipe; everything else comes
//! from the export's tables, so a user's recipe is analysed as one of VBCI's records is.
//!
//! How sure each number is comes from how it was reached, never typed: each line says how its grams
//! were reached, or why it has none; an amount the data does not pin to one number is the range of
//! what it gives (decided with Jon, 2026-10-12); and each total says how many lines it covers.

use crate::{Export, Table, Value};
use cooklang::convert::PhysicalQuantity;
use cooklang::metadata::Servings;
use cooklang::{
    Converter, CooklangParser, Extensions, IngredientReferenceTarget, Quantity, Value as Amount,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// Where VBCI's recipes are, relative to the repo root: one `<name>.cook` file each.
pub const RECIPES_DIR: &str = "recipes";

/// Where each recipe's report is written, relative to the repo root: `<name>.md`.
pub const REPORTS_DIR: &str = "docs/recipes";

/// The states a form may be in, as the records write them (crates/records, `State`). A note names
/// one by its words, "as sold" for `as_sold`.
pub const STATES: [&str; 6] = ["raw", "dry", "cooked", "canned", "frozen", "as_sold"];

/// An amount that may not be one number: its least and its greatest, equal when it is one. Every
/// amount here is 0 or more.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub low: f64,
    pub high: f64,
}

impl Range {
    fn one(x: f64) -> Range {
        Range { low: x, high: x }
    }

    /// From the least to the greatest of `xs`, which is not empty.
    fn of(xs: &[f64]) -> Range {
        Range {
            low: xs.iter().copied().fold(f64::INFINITY, f64::min),
            high: xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        }
    }

    fn times(self, k: Range) -> Range {
        Range {
            low: self.low * k.low,
            high: self.high * k.high,
        }
    }

    fn plus(self, o: Range) -> Range {
        Range {
            low: self.low + o.low,
            high: self.high + o.high,
        }
    }
}

/// How a line's grams were reached.
#[derive(Debug, Clone, PartialEq)]
pub enum How {
    /// Its unit is a mass unit: grams by the unit's definition, in cooklang's bundled units.
    Mass,
    /// The food's USDA portions in its unit, by their texts.
    Portions(Vec<String>),
    /// Of the food's USDA portions in its unit, which disagree, those whose text holds its note's
    /// words as one run, in their order.
    PortionsByNote(Vec<String>),
    /// The food has no portion in its volume unit: grams per millilitre from the food's portions in
    /// other volume units, by their texts.
    Density(Vec<String>),
    /// Its amount has no unit and its name has none (`ingredient_alias_unit`), so it counts whole
    /// items: the food's USDA portions in the count units one of which is one whole item, by their
    /// texts.
    Whole(Vec<String>),
    /// Of the food's USDA portions of one whole item, which disagree, those whose text holds its
    /// note's words as one run, in their order.
    WholeByNote(Vec<String>),
    /// Its amount has no unit, and its name is an alias ending in a count unit's name: the food's
    /// USDA portions in that unit, as `Portions` or `PortionsByNote` say.
    Named(String, Box<How>),
}

/// Why a line has no grams.
#[derive(Debug, Clone, PartialEq)]
pub enum NoGrams {
    /// Its name is no ingredient's alias.
    NoIngredient,
    /// Its note names a state its ingredient has no form in.
    NoForm(String),
    /// Its note names more than one state.
    States(Vec<String>),
    /// It has no amount.
    NoAmount,
    /// Its amount is text, such as "to taste".
    TextAmount(String),
    /// Its amount has no unit and its name has none (`ingredient_alias_unit`), so it counts whole
    /// items, and the food has no USDA portion in a count unit one of which is one whole item.
    NoWhole,
    /// Its amount has no unit, its name is an alias ending in a count unit's name, and the food
    /// has no USDA portion in that unit.
    NoNamedPortion(String),
    /// Its unit is no mass, volume or count unit.
    Unit(String),
    /// The food has no USDA portion in its unit, nor, for a volume unit, in any volume unit.
    NoPortion(String),
}

/// The food a line is, with its diet answers.
#[derive(Debug, Clone, PartialEq)]
pub struct Food {
    /// The ingredient's id.
    pub ingredient: String,
    pub state: String,
    pub fdc_id: i64,
    /// USDA's name for the food.
    pub description: String,
    pub vegan: String,
    pub vegan_basis: String,
    pub vegetarian: String,
    pub vegetarian_basis: String,
}

/// One ingredient line of a recipe.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// The ingredient's name, as written.
    pub name: String,
    /// Its amount and unit, as cooklang writes them back ("1/2 cup").
    pub amount: Option<String>,
    pub note: Option<String>,
    /// The food it is; none when its name or note finds none.
    pub food: Option<Food>,
    /// Its grams and how they were reached, or why it has none.
    pub grams: Result<(Range, How), NoGrams>,
}

/// A recipe's answer for one diet, and the lines, by position, that decided it.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// yes, no or unknown.
    pub answer: &'static str,
    pub lines: Vec<usize>,
}

/// A nutrient's total over a recipe's lines.
#[derive(Debug, Clone, PartialEq)]
pub struct Total {
    /// The nutrient's key.
    pub nutrient: String,
    pub name: String,
    pub unit: String,
    pub amount: Range,
    /// How many lines it covers: those with grams whose food has a value for the nutrient.
    pub lines: usize,
}

/// A recipe, analysed.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub title: Option<String>,
    /// Its servings, when its metadata gives a whole number.
    pub servings: Option<u32>,
    pub lines: Vec<Line>,
    pub vegan: Answer,
    pub vegetarian: Answer,
    /// One per nutrient, in the export's order.
    pub totals: Vec<Total>,
}

fn parser() -> CooklangParser {
    CooklangParser::new(Extensions::all(), Converter::bundled())
}

/// The text of each of cooklang's diagnostics.
fn diagnostics<'a>(of: impl Iterator<Item = &'a cooklang::error::SourceDiag>) -> Vec<String> {
    of.map(|d| d.to_string()).collect()
}

/// Checks a recipe record: it parses with no error and no warning, and has a title and a whole
/// number of servings, 1 or more.
pub fn check_record(text: &str) -> Result<(), String> {
    let (recipe, warnings) = parser()
        .parse(text)
        .into_result()
        .map_err(|r| diagnostics(r.errors()).join("; "))?;
    let warnings = diagnostics(warnings.warnings());
    if !warnings.is_empty() {
        return Err(format!("cooklang warns: {}", warnings.join("; ")));
    }
    if recipe.metadata.title().is_none_or(|t| t.trim().is_empty()) {
        return Err("no title".into());
    }
    match recipe.metadata.servings() {
        Some(Servings::Number(n)) if n >= 1 => Ok(()),
        other => Err(format!(
            "servings is {other:?}, not a whole number 1 or more"
        )),
    }
}

/// A cell's text; none for null or another type, as for `number` and `integer`.
pub(crate) fn text(v: &Value) -> Option<&str> {
    match v {
        Value::String(s) => Some(s),
        _ => None,
    }
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(x) => Some(*x),
        Value::Integer(x) => Some(*x as f64),
        _ => None,
    }
}

pub(crate) fn integer(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(x) => Some(*x),
        _ => None,
    }
}

/// A table of the export and the positions of the fields named.
pub(crate) fn columns<'a, const N: usize>(
    export: &'a Export,
    table: &str,
    names: [&str; N],
) -> Result<(&'a Table, [usize; N]), String> {
    let t = export
        .table(table)
        .ok_or_else(|| format!("the export has no table {table}"))?;
    let mut at = [0; N];
    for (i, n) in names.iter().enumerate() {
        at[i] = t
            .column(n)
            .ok_or_else(|| format!("the export's {table} has no field {n}"))?;
    }
    Ok((t, at))
}

/// A line's name as an alias is written: lowercased, with single spaces.
fn key(name: &str) -> String {
    name.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Lowercase words of letters and digits, each between single spaces: " cup chopped " for
/// "Cup, chopped". A phrase holds another's words, in order, when its text contains the other's.
fn phrase(s: &str) -> String {
    let words: Vec<String> = s
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    format!(" {} ", words.join(" "))
}

struct Form {
    ingredient: String,
    state: String,
    is_default: bool,
    fdc_id: i64,
}

struct Portion {
    fdc_id: i64,
    kind: String,
    unit: String,
    grams_per_unit: f64,
    grams_per_ml: Option<f64>,
    /// Its text: its unit's text, and its modifier where that differs.
    text: String,
}

/// What a line is read against, from the export's tables.
struct Kitchen {
    aliases: BTreeMap<String, String>,
    forms: Vec<Form>,
    foods: BTreeMap<i64, Food>,
    /// Each food's amount per 100 g of each nutrient it has a value for.
    amounts: BTreeMap<i64, BTreeMap<String, f64>>,
    /// Each nutrient's key, name and unit, in the export's order.
    nutrients: Vec<(String, String, String)>,
    portions: Vec<Portion>,
    /// Each count unit's names, with the unit it names.
    counts: BTreeMap<String, String>,
    /// The count units one of which is one whole item of a food.
    whole: BTreeSet<String>,
    /// Each alias ending in a count unit's name, with that unit.
    alias_units: BTreeMap<String, String>,
    converter: Converter,
}

impl Kitchen {
    fn new(export: &Export) -> Result<Kitchen, String> {
        let bad =
            |table: &str, n: usize| format!("the export's {table}: row {} lacks a value", n + 1);
        let (t, [alias, ingredient]) =
            columns(export, "ingredient_alias", ["alias", "ingredient"])?;
        let mut aliases = BTreeMap::new();
        for (n, r) in t.rows.iter().enumerate() {
            let (Some(a), Some(i)) = (text(&r[alias]), text(&r[ingredient])) else {
                return Err(bad("ingredient_alias", n));
            };
            aliases.insert(a.to_string(), i.to_string());
        }
        let (t, [ingredient, state, is_default, fdc_id]) = columns(
            export,
            "ingredient_form",
            ["ingredient", "state", "is_default", "fdc_id"],
        )?;
        let mut forms = Vec::new();
        for (n, r) in t.rows.iter().enumerate() {
            let (Some(i), Some(s), Value::Boolean(d), Some(f)) = (
                text(&r[ingredient]),
                text(&r[state]),
                &r[is_default],
                integer(&r[fdc_id]),
            ) else {
                return Err(bad("ingredient_form", n));
            };
            forms.push(Form {
                ingredient: i.to_string(),
                state: s.to_string(),
                is_default: *d,
                fdc_id: f,
            });
        }
        let (
            t,
            [
                fdc_id,
                description,
                vegan,
                vegan_basis,
                vegetarian,
                vegetarian_basis,
            ],
        ) = columns(
            export,
            "food_diet",
            [
                "fdc_id",
                "description",
                "vegan",
                "vegan_basis",
                "vegetarian",
                "vegetarian_basis",
            ],
        )?;
        let mut foods = BTreeMap::new();
        for (n, r) in t.rows.iter().enumerate() {
            let cell = |at: usize| text(&r[at]).map(str::to_string);
            let (Some(f), Some(d), Some(v), Some(vb), Some(g), Some(gb)) = (
                integer(&r[fdc_id]),
                cell(description),
                cell(vegan),
                cell(vegan_basis),
                cell(vegetarian),
                cell(vegetarian_basis),
            ) else {
                return Err(bad("food_diet", n));
            };
            foods.insert(
                f,
                Food {
                    ingredient: String::new(),
                    state: String::new(),
                    fdc_id: f,
                    description: d,
                    vegan: v,
                    vegan_basis: vb,
                    vegetarian: g,
                    vegetarian_basis: gb,
                },
            );
        }
        let (t, [fdc_id, nutrient, amount]) =
            columns(export, "food_nutrient", ["fdc_id", "nutrient", "amount"])?;
        let mut amounts: BTreeMap<i64, BTreeMap<String, f64>> = BTreeMap::new();
        for (n, r) in t.rows.iter().enumerate() {
            let (Some(f), Some(k), Some(a)) =
                (integer(&r[fdc_id]), text(&r[nutrient]), number(&r[amount]))
            else {
                return Err(bad("food_nutrient", n));
            };
            amounts.entry(f).or_default().insert(k.to_string(), a);
        }
        let (t, [nutrient, position, name, unit]) =
            columns(export, "nutrient", ["nutrient", "position", "name", "unit"])?;
        let mut nutrients = Vec::new();
        for (n, r) in t.rows.iter().enumerate() {
            let (Some(k), Some(p), Some(nm), Some(u)) = (
                text(&r[nutrient]),
                integer(&r[position]),
                text(&r[name]),
                text(&r[unit]),
            ) else {
                return Err(bad("nutrient", n));
            };
            nutrients.push((p, k.to_string(), nm.to_string(), u.to_string()));
        }
        nutrients.sort_by_key(|(p, ..)| *p);
        let nutrients = nutrients
            .into_iter()
            .map(|(_, k, n, u)| (k, n, u))
            .collect();
        let (
            t,
            [
                fdc_id,
                kind,
                unit,
                grams_per_unit,
                grams_per_ml,
                unit_text,
                modifier,
            ],
        ) = columns(
            export,
            "portion",
            [
                "fdc_id",
                "kind",
                "unit",
                "grams_per_unit",
                "grams_per_ml",
                "unit_text",
                "modifier",
            ],
        )?;
        let mut portions = Vec::new();
        for (n, r) in t.rows.iter().enumerate() {
            let (Some(f), Some(k), Some(u), Some(g), Some(ut)) = (
                integer(&r[fdc_id]),
                text(&r[kind]),
                text(&r[unit]),
                number(&r[grams_per_unit]),
                text(&r[unit_text]),
            ) else {
                return Err(bad("portion", n));
            };
            let text = match text(&r[modifier]) {
                Some(m) if m != ut => format!("{ut}, {m}"),
                _ => ut.to_string(),
            };
            portions.push(Portion {
                fdc_id: f,
                kind: k.to_string(),
                unit: u.to_string(),
                grams_per_unit: g,
                grams_per_ml: number(&r[grams_per_ml]),
                text,
            });
        }
        let (t, [name, unit]) = columns(export, "count_unit_name", ["name", "unit"])?;
        let mut counts = BTreeMap::new();
        for (n, r) in t.rows.iter().enumerate() {
            let (Some(nm), Some(u)) = (text(&r[name]), text(&r[unit])) else {
                return Err(bad("count_unit_name", n));
            };
            counts.insert(nm.to_string(), u.to_string());
        }
        let (t, [unit, whole]) = columns(export, "count_unit", ["unit", "whole"])?;
        let mut wholes = BTreeSet::new();
        for (n, r) in t.rows.iter().enumerate() {
            let (Some(u), Value::Boolean(w)) = (text(&r[unit]), &r[whole]) else {
                return Err(bad("count_unit", n));
            };
            if *w {
                wholes.insert(u.to_string());
            }
        }
        let (t, [alias, unit]) = columns(export, "ingredient_alias_unit", ["alias", "unit"])?;
        let mut alias_units = BTreeMap::new();
        for (n, r) in t.rows.iter().enumerate() {
            let (Some(a), Some(u)) = (text(&r[alias]), text(&r[unit])) else {
                return Err(bad("ingredient_alias_unit", n));
            };
            alias_units.insert(a.to_string(), u.to_string());
        }
        Ok(Kitchen {
            aliases,
            forms,
            foods,
            amounts,
            nutrients,
            portions,
            counts,
            whole: wholes,
            alias_units,
            converter: Converter::bundled(),
        })
    }

    /// The food a line's name and note find, or why there is none.
    fn food(&self, name: &str, note: &str) -> Result<Food, NoGrams> {
        let ingredient = self.aliases.get(&key(name)).ok_or(NoGrams::NoIngredient)?;
        let note = phrase(note);
        let named: Vec<&str> = STATES
            .into_iter()
            .filter(|s| note.contains(&phrase(&s.replace('_', " "))))
            .collect();
        let form = match named[..] {
            [] => self
                .forms
                .iter()
                .find(|f| &f.ingredient == ingredient && f.is_default),
            [state] => self
                .forms
                .iter()
                .find(|f| &f.ingredient == ingredient && f.state == state),
            _ => {
                return Err(NoGrams::States(
                    named.iter().map(|s| s.to_string()).collect(),
                ));
            }
        };
        let form = form.ok_or_else(|| NoGrams::NoForm(named.concat()))?;
        let food = self
            .foods
            .get(&form.fdc_id)
            .ok_or_else(|| NoGrams::NoForm(form.state.clone()))?;
        Ok(Food {
            ingredient: ingredient.clone(),
            state: form.state.clone(),
            ..food.clone()
        })
    }

    /// The grams of `fdc_id` a line's quantity gives, and how, or why it gives none. `name` is the
    /// line's name, an alias of the food's ingredient.
    fn grams(
        &self,
        fdc_id: i64,
        quantity: Option<&Quantity>,
        note: &str,
        name: &str,
    ) -> Result<(Range, How), NoGrams> {
        let q = quantity.ok_or(NoGrams::NoAmount)?;
        let amount = match q.value() {
            Amount::Number(n) => Range::one(n.value()),
            Amount::Range { start, end } => Range {
                low: start.value(),
                high: end.value(),
            },
            Amount::Text(t) => return Err(NoGrams::TextAmount(t.clone())),
        };
        let Some(unit) = q.unit() else {
            if let Some(named) = self.alias_units.get(&key(name)) {
                let (per, texts, noted) = self
                    .portions(fdc_id, "count", |u| u == named, note)
                    .ok_or_else(|| NoGrams::NoNamedPortion(named.clone()))?;
                let how = How::Named(named.clone(), Box::new(in_unit(texts, noted)));
                return Ok((amount.times(per), how));
            }
            let (per, texts, noted) = self
                .portions(fdc_id, "count", |u| self.whole.contains(u), note)
                .ok_or(NoGrams::NoWhole)?;
            let how = if noted {
                How::WholeByNote(texts)
            } else {
                How::Whole(texts)
            };
            return Ok((amount.times(per), how));
        };
        // A unit is read as the warehouse reads a portion's (crates/warehouse, src/portions.rs):
        // lowercased with single spaces, a mass or volume unit cooklang knows, by its first symbol,
        // else a count unit's name.
        let name = unit
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if let Some(u) = self.converter.find_unit(&name) {
            let symbol = u.symbols.first().or(u.names.first()).map(|s| s.to_string());
            let symbol = symbol.unwrap_or_default();
            return match u.physical_quantity {
                PhysicalQuantity::Mass => Ok((amount.times(Range::one(u.ratio)), How::Mass)),
                PhysicalQuantity::Volume => {
                    if let Some((per, texts, noted)) =
                        self.portions(fdc_id, "volume", |u| u == symbol, note)
                    {
                        return Ok((amount.times(per), in_unit(texts, noted)));
                    }
                    let volume: Vec<&Portion> = self
                        .portions
                        .iter()
                        .filter(|p| p.fdc_id == fdc_id && p.kind == "volume")
                        .collect();
                    let per_ml: Vec<f64> = volume.iter().filter_map(|p| p.grams_per_ml).collect();
                    if per_ml.is_empty() {
                        return Err(NoGrams::NoPortion(unit.to_string()));
                    }
                    let ml = u.ratio * 1000.0;
                    Ok((
                        amount.times(Range::one(ml)).times(Range::of(&per_ml)),
                        How::Density(volume.iter().map(|p| p.text.clone()).collect()),
                    ))
                }
                _ => Err(NoGrams::Unit(unit.to_string())),
            };
        }
        let count = self
            .counts
            .get(&name)
            .ok_or_else(|| NoGrams::Unit(unit.to_string()))?;
        let (per, texts, noted) = self
            .portions(fdc_id, "count", |u| u == count, note)
            .ok_or_else(|| NoGrams::NoPortion(unit.to_string()))?;
        Ok((amount.times(per), in_unit(texts, noted)))
    }

    /// Grams in one unit of `fdc_id` by its USDA portions of `kind` in the units `unit` accepts:
    /// the range of what they give, or, when they disagree, of those whose text holds the note's
    /// words as one run, in their order, if some do and some do not; with those portions' texts,
    /// and whether the note's words chose them. None when it has no such portion.
    fn portions(
        &self,
        fdc_id: i64,
        kind: &str,
        unit: impl Fn(&str) -> bool,
        note: &str,
    ) -> Option<(Range, Vec<String>, bool)> {
        let all: Vec<&Portion> = self
            .portions
            .iter()
            .filter(|p| p.fdc_id == fdc_id && p.kind == kind && unit(&p.unit))
            .collect();
        if all.is_empty() {
            return None;
        }
        let per =
            |ps: &[&Portion]| Range::of(&ps.iter().map(|p| p.grams_per_unit).collect::<Vec<_>>());
        let texts = |ps: &[&Portion]| ps.iter().map(|p| p.text.clone()).collect::<Vec<_>>();
        let range = per(&all);
        let note = phrase(note);
        if range.low != range.high && note.trim() != "" {
            let named: Vec<&Portion> = all
                .iter()
                .copied()
                .filter(|p| phrase(&p.text).contains(&note))
                .collect();
            if !named.is_empty() && named.len() < all.len() {
                return Some((per(&named), texts(&named), true));
            }
        }
        Some((range, texts(&all), false))
    }
}

/// How a line's grams were reached by the food's portions in its unit, whether its note's words
/// chose them or not.
fn in_unit(texts: Vec<String>, noted: bool) -> How {
    if noted {
        How::PortionsByNote(texts)
    } else {
        How::Portions(texts)
    }
}

/// The weakest of the lines' answers: no if one is, else unknown if one is or a line has no food,
/// else yes; with the lines that decided it. A recipe with no line is unknown.
fn weakest(lines: &[Line], answer: impl Fn(&Food) -> &str) -> Answer {
    let answers: Vec<&str> = lines
        .iter()
        .map(|l| l.food.as_ref().map_or("unknown", &answer))
        .collect();
    if answers.is_empty() {
        return Answer {
            answer: "unknown",
            lines: Vec::new(),
        };
    }
    for weak in ["no", "unknown"] {
        let at: Vec<usize> = (0..answers.len()).filter(|&i| answers[i] == weak).collect();
        if !at.is_empty() {
            return Answer {
                answer: weak,
                lines: at,
            };
        }
    }
    Answer {
        answer: "yes",
        lines: (0..answers.len()).collect(),
    }
}

/// Analyses a recipe's text against the export: every ingredient line, and each reference to an
/// earlier one that gives an amount of its own. Err when cooklang refuses the text.
pub fn analyze(text: &str, export: &Export) -> Result<Report, String> {
    let (recipe, _) = parser()
        .parse(text)
        .into_result()
        .map_err(|r| diagnostics(r.errors()).join("; "))?;
    let kitchen = Kitchen::new(export)?;
    let mut lines = Vec::new();
    for ing in &recipe.ingredients {
        if !ing.relation.is_definition() && ing.quantity.is_none() {
            continue;
        }
        // A reference is its definition's food, so it reads the definition's note.
        let note = match ing.relation.references_to() {
            Some((at, IngredientReferenceTarget::Ingredient)) => &recipe.ingredients[at].note,
            _ => &ing.note,
        };
        let note = note.clone().unwrap_or_default();
        let (food, grams) = match kitchen.food(&ing.name, &note) {
            Ok(f) => {
                let g = kitchen.grams(f.fdc_id, ing.quantity.as_ref(), &note, &ing.name);
                (Some(f), g)
            }
            Err(why) => (None, Err(why)),
        };
        lines.push(Line {
            name: ing.name.clone(),
            amount: ing.quantity.as_ref().map(|q| q.to_string()),
            note: ing.note.clone(),
            food,
            grams,
        });
    }
    let mut totals = Vec::new();
    for (key, name, unit) in &kitchen.nutrients {
        let mut amount = Range::one(0.0);
        let mut covered = 0;
        for l in &lines {
            let (Some(food), Ok((grams, _))) = (&l.food, &l.grams) else {
                continue;
            };
            if let Some(per_100) = kitchen.amounts.get(&food.fdc_id).and_then(|a| a.get(key)) {
                amount = amount.plus(grams.times(Range::one(per_100 / 100.0)));
                covered += 1;
            }
        }
        totals.push(Total {
            nutrient: key.clone(),
            name: name.clone(),
            unit: unit.clone(),
            amount,
            lines: covered,
        });
    }
    Ok(Report {
        title: recipe.metadata.title().map(str::to_string),
        servings: match recipe.metadata.servings() {
            Some(Servings::Number(n)) if n >= 1 => Some(n),
            _ => None,
        },
        vegan: weakest(&lines, |f| &f.vegan),
        vegetarian: weakest(&lines, |f| &f.vegetarian),
        lines,
        totals,
    })
}

/// An amount as a report shows it: to 0.1, and "<0.1" for one above 0 that rounds to 0.0.
pub fn shown(x: f64) -> String {
    let s = format!("{x:.1}");
    if s == "0.0" && x > 0.0 {
        "<0.1".into()
    } else {
        s
    }
}

/// A range as a report shows it: one amount when both ends show the same, else "low–high".
pub fn shown_range(r: Range) -> String {
    let (low, high) = (shown(r.low), shown(r.high));
    if low == high {
        low
    } else {
        format!("{low}–{high}")
    }
}

/// A Markdown table cell's text.
fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

/// USDA's texts, each in backticks, joined by semicolons.
fn code(texts: &[String]) -> String {
    texts
        .iter()
        .map(|t| format!("`{t}`"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn state(s: &str) -> String {
    s.replace('_', " ")
}

impl How {
    fn said(&self) -> String {
        match self {
            How::Mass => "by the unit's definition".into(),
            How::Portions(t) if t.len() == 1 => format!("USDA's portion {}", code(t)),
            How::Portions(t) => format!("USDA's portions {}", code(t)),
            How::PortionsByNote(t) if t.len() == 1 => {
                format!("USDA's portion the note names, {}", code(t))
            }
            How::PortionsByNote(t) => format!("USDA's portions the note names, {}", code(t)),
            How::Density(t) => format!("grams per millilitre from USDA's {}", code(t)),
            How::Whole(t) if t.len() == 1 => {
                format!("whole items, by USDA's portion {}", code(t))
            }
            How::Whole(t) => format!("whole items, by USDA's portions {}", code(t)),
            How::WholeByNote(t) if t.len() == 1 => {
                format!("whole items, by USDA's portion the note names, {}", code(t))
            }
            How::WholeByNote(t) => {
                format!(
                    "whole items, by USDA's portions the note names, {}",
                    code(t)
                )
            }
            How::Named(u, how) => format!("`{u}`, its name's unit: {}", how.said()),
        }
    }
}

impl NoGrams {
    fn said(&self) -> String {
        match self {
            NoGrams::NoIngredient => "its name is no ingredient's alias".into(),
            NoGrams::NoForm(s) => format!("its ingredient has no form {}", state(s)),
            NoGrams::States(s) => format!(
                "its note names more than one state: {}",
                s.iter().map(|s| state(s)).collect::<Vec<_>>().join(", ")
            ),
            NoGrams::NoAmount => "it has no amount".into(),
            NoGrams::TextAmount(t) => format!("its amount is text, `{t}`"),
            NoGrams::NoWhole => {
                "its amount has no unit, and the food has no USDA portion of one whole item".into()
            }
            NoGrams::NoNamedPortion(u) => {
                format!("its name's unit is `{u}`, and the food has no USDA portion in it")
            }
            NoGrams::Unit(u) => format!("`{u}` is no mass, volume or count unit"),
            NoGrams::NoPortion(u) => format!("the food has no USDA portion in `{u}`"),
        }
    }
}

impl Line {
    /// The line as a report names it: "red lentils, 1 cup (dry)".
    fn said(&self) -> String {
        let mut s = self.name.clone();
        if let Some(a) = &self.amount {
            s.push_str(&format!(", {a}"));
        }
        if let Some(n) = &self.note {
            s.push_str(&format!(" ({n})"));
        }
        s
    }
}

impl Report {
    /// The report's Markdown, for the recipe at `path`: its title, `origin`, a paragraph saying
    /// where the report came from, and the rest.
    pub fn markdown(&self, path: &str, origin: &str) -> String {
        let mut out = format!(
            "# {}\n\n{origin}\n\n\
             How sure each number is comes from how it was reached: each line says how its grams\n\
             were reached, or why it has none; an amount the data does not pin to one number is\n\
             the range of what it gives; and each total says how many of the recipe's lines it\n\
             covers. Grams and amounts are rounded to 0.1, and one above 0 that rounds to 0.0 is\n\
             shown as <0.1.\n\n",
            self.title.as_deref().unwrap_or(path)
        );
        let with_grams = self.lines.iter().filter(|l| l.grams.is_ok()).count();
        if let Some(n) = self.servings {
            out.push_str(&format!("Serves {n}. "));
        }
        out.push_str(&format!(
            "Ingredient lines: {}, with grams: {with_grams}.\n\n## Diet\n\n\
             Vegan is The Vegan Society's definition and vegetarian the Vegetarian Society's\n\
             criteria (`docs/architecture.md`, \"Decided\"). A recipe's answer is the weakest of\n\
             its lines': no if one is, else unknown if one is or a line has no food, else yes.\n\n\
             | diet | answer | decided by |\n|---|---|---|\n",
            self.lines.len()
        ));
        for (diet, a) in [("vegan", &self.vegan), ("vegetarian", &self.vegetarian)] {
            let basis = |f: &Food| match diet {
                "vegan" => f.vegan_basis.clone(),
                _ => f.vegetarian_basis.clone(),
            };
            let by = if a.answer == "yes" {
                "every line".to_string()
            } else if a.lines.is_empty() {
                "no line".to_string()
            } else {
                a.lines
                    .iter()
                    .map(|&i| {
                        let l = &self.lines[i];
                        match &l.food {
                            Some(f) => format!("{} ({})", l.name, basis(f)),
                            None => format!("{} (no food)", l.name),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            out.push_str(&format!("| {diet} | {} | {} |\n", a.answer, cell(&by)));
        }
        out.push_str("\n## Lines\n\n| line | food | grams | how the grams were reached |\n");
        out.push_str("|---|---|---|---|\n");
        for l in &self.lines {
            let food = match &l.food {
                Some(f) => format!(
                    "{}, {}: {} ({})",
                    f.ingredient,
                    state(&f.state),
                    f.description,
                    f.fdc_id
                ),
                None => "none".into(),
            };
            let (grams, how) = match &l.grams {
                Ok((g, how)) => (shown_range(*g), how.said()),
                Err(why) => ("none".into(), why.said()),
            };
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                cell(&l.said()),
                cell(&food),
                grams,
                cell(&how)
            ));
        }
        out.push_str("\n## Nutrients\n\n");
        match self.servings {
            Some(_) => {
                out.push_str("| nutrient | unit | total | per serving | lines it covers |\n");
                out.push_str("|---|---|---|---|---|\n");
            }
            None => {
                out.push_str("| nutrient | unit | total | lines it covers |\n");
                out.push_str("|---|---|---|---|\n");
            }
        }
        for t in &self.totals {
            let per = match self.servings {
                Some(n) => format!(
                    " {} |",
                    shown_range(t.amount.times(Range::one(1.0 / n as f64)))
                ),
                None => String::new(),
            };
            out.push_str(&format!(
                "| {} | {} | {} |{per} {} of {} |\n",
                cell(&t.name),
                t.unit,
                shown_range(t.amount),
                t.lines,
                self.lines.len()
            ));
        }
        out.push_str("\n## Lines without grams\n\n");
        let without: Vec<&Line> = self.lines.iter().filter(|l| l.grams.is_err()).collect();
        if without.is_empty() {
            out.push_str("None: every line has grams.\n");
        }
        for l in without {
            if let Err(why) = &l.grams {
                out.push_str(&format!("- {}: {}.\n", l.said(), why.said()));
            }
        }
        out
    }
}

/// The names of the recipes in `recipes/` under `root`, sorted: each file's name less `.cook`.
/// Err on a file there that is not a `.cook` file.
pub fn recipe_names(root: &Path) -> Result<Vec<String>, String> {
    let dir = root.join(RECIPES_DIR);
    let mut names = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| format!("{RECIPES_DIR}: {e}"))? {
        let file = entry.map_err(|e| e.to_string())?.file_name();
        let file = file.to_string_lossy();
        match file.strip_suffix(".cook") {
            Some(name) if !name.is_empty() => names.push(name.to_string()),
            _ => return Err(format!("{RECIPES_DIR}/{file} is not a .cook file")),
        }
    }
    names.sort();
    Ok(names)
}

/// What a report in `docs/recipes/` says of where it came from.
fn generated(path: &str) -> String {
    format!(
        "Generated by `cargo run -p balaena-warehouse -- recipes` from `{path}`\n\
         and `export/datapackage.json`; the test `the_recipe_reports_are_current` fails\n\
         when this file differs from what that would write. Change the recipe or the records,\n\
         not this file."
    )
}

/// Each recipe's report, its path and text, for the recipes in `recipes/` under `root` read
/// against the export there: what `cargo run -p balaena-warehouse -- recipes` writes.
pub fn reports(root: &Path) -> Result<Vec<(String, String)>, String> {
    let export = Export::load(root)?;
    let mut out = Vec::new();
    for name in recipe_names(root)? {
        let path = format!("{RECIPES_DIR}/{name}.cook");
        let text = fs::read_to_string(root.join(&path)).map_err(|e| format!("{path}: {e}"))?;
        let report = analyze(&text, &export).map_err(|e| format!("{path}: {e}"))?;
        out.push((
            format!("{REPORTS_DIR}/{name}.md"),
            report.markdown(&path, &generated(&path)),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Field, Lineage, Type};

    fn table(name: &str, fields: &[(&str, Type)], rows: Vec<Vec<Value>>) -> Table {
        Table {
            name: name.into(),
            description: String::new(),
            fields: fields
                .iter()
                .map(|(n, kind)| Field {
                    name: n.to_string(),
                    kind: *kind,
                    description: None,
                })
                .collect(),
            primary_key: Vec::new(),
            foreign_keys: Vec::new(),
            rows,
        }
    }

    fn s(x: &str) -> Value {
        Value::String(x.into())
    }

    fn i(x: i64) -> Value {
        Value::Integer(x)
    }

    fn n(x: f64) -> Value {
        Value::Number(x)
    }

    /// A small export: seven foods, the portions and nutrients these tests read, and the
    /// ingredients that link them. Garlic's two clove portions agree; raw onion's two cup portions
    /// do not; lemon juice has no tablespoon portion; red lentils have no value for sugars.
    fn kitchen() -> Export {
        use Type::{Boolean, Integer, Number, String as Text};
        let food = |id: i64, d: &str, vegan: &str, vb: &str, veg: &str, gb: &str| {
            vec![
                i(id),
                s("sr-legacy"),
                s(d),
                s("c"),
                s(vegan),
                s(vb),
                s(veg),
                s(gb),
            ]
        };
        let plant = "category: plant";
        let food_diet = table(
            "food_diet",
            &[
                ("fdc_id", Integer),
                ("dataset", Text),
                ("description", Text),
                ("category", Text),
                ("vegan", Text),
                ("vegan_basis", Text),
                ("vegetarian", Text),
                ("vegetarian_basis", Text),
            ],
            vec![
                food(1, "Garlic, raw", "yes", plant, "yes", plant),
                food(2, "Onions, raw", "yes", plant, "yes", plant),
                food(3, "Onions, cooked", "yes", plant, "yes", plant),
                food(4, "Lemon juice, raw", "yes", plant, "yes", plant),
                food(
                    5,
                    "Cheese, parmesan, grated",
                    "no",
                    "category: dairy_and_egg",
                    "unknown",
                    "rule: Cheese",
                ),
                food(6, "Lentils, pink or red, raw", "yes", plant, "yes", plant),
                food(
                    7,
                    "Soup, vegetable broth",
                    "unknown",
                    "category: mixed",
                    "unknown",
                    "category: mixed",
                ),
            ],
        );
        // Listed out of order: totals follow position.
        let nutrient = table(
            "nutrient",
            &[
                ("nutrient", Text),
                ("position", Integer),
                ("name", Text),
                ("unit", Text),
            ],
            vec![
                vec![s("sugars"), i(2), s("Sugars, total"), s("G")],
                vec![s("energy"), i(1), s("Energy"), s("KCAL")],
            ],
        );
        let amount = |id: i64, k: &str, a: f64| vec![i(id), s(k), n(a)];
        let food_nutrient = table(
            "food_nutrient",
            &[("fdc_id", Integer), ("nutrient", Text), ("amount", Number)],
            vec![
                amount(1, "energy", 149.0),
                amount(1, "sugars", 1.0),
                amount(2, "energy", 40.0),
                amount(2, "sugars", 4.24),
                amount(3, "energy", 44.0),
                amount(4, "energy", 22.0),
                amount(5, "energy", 420.0),
                amount(6, "energy", 358.0),
                amount(7, "energy", 5.0),
            ],
        );
        let (cup, tsp) = (236.588236, 4.928921);
        let portion = |id: i64, kind: &str, unit: &str, g: f64, ml: Option<f64>, t: &str| {
            vec![
                i(id),
                s(kind),
                s(unit),
                n(g),
                ml.map_or(Value::Null, |m| n(g / m)),
                s(t),
                s(t),
            ]
        };
        let mut portions = vec![
            portion(1, "count", "clove", 3.0, None, "clove"),
            portion(1, "count", "clove", 3.0, None, "cloves"),
            portion(1, "volume", "tsp", 2.8, Some(tsp), "tsp"),
            portion(2, "volume", "c", 160.0, Some(cup), "cup, chopped"),
            portion(2, "volume", "c", 115.0, Some(cup), "cup, sliced"),
            portion(2, "count", "small", 70.0, None, "small"),
            portion(2, "count", "medium", 110.0, None, "medium (2-1/2\" dia)"),
            portion(2, "count", "large", 150.0, None, "large"),
            portion(
                2,
                "count",
                "slice",
                14.0,
                None,
                "slice, medium (1/8\" thick)",
            ),
            portion(
                2,
                "count",
                "slice",
                38.0,
                None,
                "slice, large (1/4\" thick)",
            ),
            portion(3, "count", "medium", 94.0, None, "medium"),
            portion(5, "mass", "oz", 28.35, None, "oz"),
            portion(6, "volume", "c", 192.0, Some(cup), "cup"),
            portion(7, "volume", "c", 221.0, Some(cup), "cup"),
        ];
        // Foundation's kind: a measure unit's name, and no modifier.
        for (unit, t, g, ml) in [
            ("c", "cup", 244.0, cup),
            ("fl oz", "fl oz", 30.5, 29.573529),
        ] {
            portions.push(vec![
                i(4),
                s("volume"),
                s(unit),
                n(g),
                n(g / ml),
                s(t),
                Value::Null,
            ]);
        }
        let portion = table(
            "portion",
            &[
                ("fdc_id", Integer),
                ("kind", Text),
                ("unit", Text),
                ("grams_per_unit", Number),
                ("grams_per_ml", Number),
                ("unit_text", Text),
                ("modifier", Text),
            ],
            portions,
        );
        let count_unit = table(
            "count_unit",
            &[("unit", Text), ("whole", Boolean)],
            [
                ("clove", false),
                ("large", true),
                ("medium", true),
                ("slice", false),
                ("small", true),
            ]
            .iter()
            .map(|(u, w)| vec![s(u), Value::Boolean(*w)])
            .collect(),
        );
        let count_unit_name = table(
            "count_unit_name",
            &[("name", Text), ("unit", Text)],
            vec![
                vec![s("clove"), s("clove")],
                vec![s("cloves"), s("clove")],
                vec![s("medium"), s("medium")],
                vec![s("slice"), s("slice")],
            ],
        );
        let ingredient_alias = table(
            "ingredient_alias",
            &[("alias", Text), ("ingredient", Text)],
            [
                ("garlic", "garlic"),
                ("garlic cloves", "garlic"),
                ("garlic slices", "garlic"),
                ("onion", "onion"),
                ("onion slices", "onion"),
                ("lemon juice", "lemon-juice"),
                ("parmesan", "parmesan"),
                ("red lentils", "red-lentils"),
                ("vegetable broth", "vegetable-broth"),
            ]
            .iter()
            .map(|(a, b)| vec![s(a), s(b)])
            .collect(),
        );
        let ingredient_alias_unit = table(
            "ingredient_alias_unit",
            &[("alias", Text), ("unit", Text)],
            [
                ("garlic cloves", "clove"),
                ("garlic slices", "slice"),
                ("onion slices", "slice"),
            ]
            .iter()
            .map(|(a, u)| vec![s(a), s(u)])
            .collect(),
        );
        let ingredient_form = table(
            "ingredient_form",
            &[
                ("ingredient", Text),
                ("state", Text),
                ("is_default", Boolean),
                ("fdc_id", Integer),
            ],
            // Onion's default listed second: the default is the form marked so, not the first.
            [
                ("garlic", "raw", true, 1),
                ("onion", "cooked", false, 3),
                ("onion", "raw", true, 2),
                ("lemon-juice", "raw", true, 4),
                ("parmesan", "as_sold", true, 5),
                ("red-lentils", "dry", true, 6),
                ("vegetable-broth", "as_sold", true, 7),
            ]
            .iter()
            .map(|(a, st, d, f)| vec![s(a), s(st), Value::Boolean(*d), i(*f)])
            .collect(),
        );
        Export {
            name: "kitchen".into(),
            title: String::new(),
            description: String::new(),
            licenses: Vec::new(),
            sources: Vec::new(),
            lineage: Lineage {
                zips: Vec::new(),
                wikidata: Vec::new(),
                records: Vec::new(),
            },
            tables: vec![
                food_diet,
                nutrient,
                food_nutrient,
                portion,
                count_unit,
                count_unit_name,
                ingredient_alias,
                ingredient_alias_unit,
                ingredient_form,
            ],
        }
    }

    fn report(src: &str) -> Report {
        analyze(src, &kitchen()).unwrap()
    }

    /// The one line of a one-line recipe.
    fn line(src: &str) -> Line {
        let r = report(src);
        assert_eq!(r.lines.len(), 1, "{src}");
        r.lines[0].clone()
    }

    fn grams(src: &str) -> Result<(Range, How), NoGrams> {
        line(src).grams
    }

    fn texts(t: &[&str]) -> Vec<String> {
        t.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_line_s_food_is_its_alias_s_ingredient_in_the_note_s_state() {
        let food = |src: &str| line(src).food.map(|f| (f.ingredient, f.state, f.fdc_id));
        let is = |i: &str, st: &str, f: i64| Some((i.to_string(), st.to_string(), f));
        assert_eq!(
            food("@Onion{1%medium}"),
            is("onion", "raw", 2),
            "the default"
        );
        assert_eq!(food("@onion{1%medium}(cooked)"), is("onion", "cooked", 3));
        assert_eq!(
            food("@onion{1%medium}(Cooked, sliced)"),
            is("onion", "cooked", 3)
        );
        assert_eq!(
            food("@onion{1%medium}(precooked)"),
            is("onion", "raw", 2),
            "a state is named by a whole word"
        );
        assert_eq!(
            food("@red lentils{1%cup}(dry)"),
            is("red-lentils", "dry", 6)
        );
        assert_eq!(
            food("@vegetable broth{1%cup}(as sold)"),
            is("vegetable-broth", "as_sold", 7)
        );
        let none = |src: &str, why: NoGrams| {
            let l = line(src);
            assert_eq!((l.food, l.grams), (None, Err(why)), "{src}");
        };
        let referred = report("@onion{1%medium}(cooked), then @&onion{1%medium}.").lines;
        assert_eq!(
            referred[1].food.as_ref().map(|f| f.state.as_str()),
            Some("cooked"),
            "a reference reads its definition's note"
        );
        none("@salt{1%tsp}", NoGrams::NoIngredient);
        none("@onions{1%medium}", NoGrams::NoIngredient);
        none(
            "@red lentils{1%cup}(cooked)",
            NoGrams::NoForm("cooked".into()),
        );
        none(
            "@onion{1%cup}(raw or cooked)",
            NoGrams::States(texts(&["raw", "cooked"])),
        );
    }

    #[test]
    fn each_way_a_line_s_grams_are_reached() {
        let one = |x: f64| Range::one(x);
        let both = |low: f64, high: f64| Range { low, high };
        assert_eq!(grams("@parmesan{30%g}"), Ok((one(30.0), How::Mass)));
        assert_eq!(
            grams("@parmesan{1%oz}"),
            Ok((one(28.349523125), How::Mass)),
            "by the unit's definition, not USDA's 28.35 g portion"
        );
        assert_eq!(
            grams("@garlic{3%cloves}"),
            Ok((one(9.0), How::Portions(texts(&["clove", "cloves"])))),
            "portions that agree"
        );
        assert_eq!(
            grams("@garlic{3%cloves}(cloves)"),
            Ok((one(9.0), How::Portions(texts(&["clove", "cloves"])))),
            "portions that agree, whatever the note names"
        );
        assert_eq!(
            grams("@garlic{2-3%cloves}"),
            Ok((both(6.0, 9.0), How::Portions(texts(&["clove", "cloves"])))),
            "a range amount"
        );
        assert_eq!(
            grams("@garlic{1%tsp}"),
            Ok((one(2.8), How::Portions(texts(&["tsp"])))),
            "a volume portion in the line's unit"
        );
        assert_eq!(
            grams("@garlic{1%Tsp}"),
            Ok((one(2.8), How::Portions(texts(&["tsp"])))),
            "a unit read lowercased, as a portion's is"
        );
        assert_eq!(
            grams("@onion{1%medium}"),
            Ok((one(110.0), How::Portions(texts(&["medium (2-1/2\" dia)"]))))
        );
        let cups = How::Portions(texts(&["cup, chopped", "cup, sliced"]));
        assert_eq!(
            grams("@onion{1%cup}"),
            Ok((both(115.0, 160.0), cups.clone())),
            "portions that disagree, and no note"
        );
        assert_eq!(
            grams("@onion{1%cup}(chopped)"),
            Ok((one(160.0), How::PortionsByNote(texts(&["cup, chopped"])))),
            "the note names one"
        );
        assert_eq!(
            grams("@onion{1%cup}(finely chopped)"),
            Ok((both(115.0, 160.0), cups)),
            "the note's words are not, as one run, in either portion's text"
        );
        assert_eq!(
            grams("@red lentils{1/2%cup}"),
            Ok((one(96.0), How::Portions(texts(&["cup"]))))
        );
        let whole = How::Whole(texts(&["small", "medium (2-1/2\" dia)", "large"]));
        assert_eq!(
            grams("@onion{1}"),
            Ok((both(70.0, 150.0), whole.clone())),
            "no unit: whole items, by the portions in units marked whole, not the slice"
        );
        assert_eq!(
            grams("@onion{1-2}"),
            Ok((both(70.0, 300.0), whole.clone())),
            "a range of whole items"
        );
        assert_eq!(
            grams("@onion{1}(chopped)"),
            Ok((both(70.0, 150.0), whole)),
            "the note's words are in no whole item's text"
        );
        assert_eq!(
            grams("@onion{1}(medium)"),
            Ok((
                one(110.0),
                How::WholeByNote(texts(&["medium (2-1/2\" dia)"]))
            )),
            "the note names one whole item; the slice's text holds it too, but a slice is not whole"
        );
        assert_eq!(
            grams("@onion{2}(cooked)"),
            Ok((one(188.0), How::Whole(texts(&["medium"])))),
            "the cooked form's one whole item"
        );
        let cloves = How::Named(
            "clove".into(),
            Box::new(How::Portions(texts(&["clove", "cloves"]))),
        );
        assert_eq!(
            grams("@garlic cloves{2}"),
            Ok((one(6.0), cloves.clone())),
            "no unit, and an alias ending in a count unit's name: that unit"
        );
        assert_eq!(
            grams("@Garlic  Cloves{2}"),
            Ok((one(6.0), cloves)),
            "the name read as an alias is"
        );
        assert_eq!(
            grams("@garlic cloves{1%tsp}"),
            Ok((one(2.8), How::Portions(texts(&["tsp"])))),
            "a unit the line gives wins over its name's"
        );
        let slices = texts(&["slice, medium (1/8\" thick)", "slice, large (1/4\" thick)"]);
        assert_eq!(
            grams("@onion slices{3}"),
            Ok((
                both(42.0, 114.0),
                How::Named("slice".into(), Box::new(How::Portions(slices.clone())))
            )),
            "its name's unit, not whole items"
        );
        assert_eq!(
            grams("@onion slices{1}(large)"),
            Ok((
                one(38.0),
                How::Named(
                    "slice".into(),
                    Box::new(How::PortionsByNote(texts(&["slice, large (1/4\" thick)"])))
                )
            )),
            "the note names one of its name's unit's portions"
        );
        assert_eq!(
            grams("@onion{1%slice}"),
            Ok((both(14.0, 38.0), How::Portions(slices))),
            "a unit not marked whole is still read in its own unit"
        );
        let (g, how) = grams("@lemon juice{2%tbsp}").unwrap();
        assert_eq!(how, How::Density(texts(&["cup", "fl oz"])));
        let tbsp: f64 = 2.0 * 14.786764;
        let (fl_oz, cup) = (tbsp * 30.5 / 29.573529, tbsp * 244.0 / 236.588236);
        assert!((g.low - fl_oz.min(cup)).abs() < 1e-9, "{g:?}");
        assert!((g.high - fl_oz.max(cup)).abs() < 1e-9, "{g:?}");
        assert_eq!(shown_range(g), "30.5");
    }

    #[test]
    fn a_line_without_grams_says_why() {
        let why = |src: &str, no: NoGrams| {
            let l = line(src);
            assert!(l.food.is_some(), "{src} has a food");
            assert_eq!(l.grams, Err(no), "{src}");
        };
        why("@garlic{}", NoGrams::NoAmount);
        why("@garlic", NoGrams::NoAmount);
        why("@garlic{to taste}", NoGrams::TextAmount("to taste".into()));
        why("@garlic{1}", NoGrams::NoWhole);
        why("@garlic slices{2}", NoGrams::NoNamedPortion("slice".into()));
        why(
            "@onion slices{2}(cooked)",
            NoGrams::NoNamedPortion("slice".into()),
        );
        why("@garlic{1%pinch}", NoGrams::Unit("pinch".into()));
        why("@garlic{2%cm}", NoGrams::Unit("cm".into()));
        why("@red lentils{1%clove}", NoGrams::NoPortion("clove".into()));
        why("@parmesan{1%cup}", NoGrams::NoPortion("cup".into()));
        let err = analyze("@onion{1}(a) and @&onion{1}(b).", &kitchen()).unwrap_err();
        assert!(err.contains("Note not allowed in reference"), "{err}");
    }

    #[test]
    fn a_recipe_s_diet_is_its_weakest_line_s() {
        let diet = |src: &str| {
            let r = report(src);
            (
                (r.vegan.answer, r.vegan.lines),
                (r.vegetarian.answer, r.vegetarian.lines),
            )
        };
        assert_eq!(
            diet("@garlic{1%clove} @onion{1%medium}"),
            (("yes", vec![0, 1]), ("yes", vec![0, 1]))
        );
        assert_eq!(
            diet("@garlic{1%clove} @vegetable broth{1%cup} @parmesan{30%g}"),
            (("no", vec![2]), ("unknown", vec![1, 2]))
        );
        assert_eq!(
            diet("@garlic{1%clove} @salt{1%tsp}"),
            (("unknown", vec![1]), ("unknown", vec![1])),
            "a line with no food"
        );
        assert_eq!(
            diet("@onion{1} @garlic{to taste}"),
            (("yes", vec![0, 1]), ("yes", vec![0, 1])),
            "lines without grams still have a food"
        );
        assert_eq!(
            diet("Boil the water."),
            (("unknown", vec![]), ("unknown", vec![])),
            "no line"
        );
    }

    #[test]
    fn a_total_says_how_many_lines_it_covers() {
        let r = report("@red lentils{1%cup} @garlic{3%cloves} @salt{1%tsp} @garlic{1}");
        let totals: Vec<(&str, usize)> = r
            .totals
            .iter()
            .map(|t| (t.nutrient.as_str(), t.lines))
            .collect();
        assert_eq!(totals, [("energy", 2), ("sugars", 1)]);
        let energy = r.totals[0].amount;
        let expected = 192.0 * 3.58 + 9.0 * 1.49;
        assert!((energy.low - expected).abs() < 1e-9, "{energy:?}");
        assert_eq!(energy.low, energy.high);
        assert!((r.totals[1].amount.low - 0.09).abs() < 1e-12);
        let refs = |src: &str| report(src).lines.len();
        assert_eq!(
            refs("@onion{1%medium}, then @&onion."),
            1,
            "a reference without an amount is its line's, not a line"
        );
        assert_eq!(
            refs("@onion{1%medium}, then @&onion{1%medium}."),
            2,
            "a reference's own amount is a line"
        );
        let onion = report("@onion{1%cup}").totals[0].amount;
        assert!((onion.low - 46.0).abs() < 1e-9 && (onion.high - 64.0).abs() < 1e-9);
    }

    #[test]
    fn amounts_are_shown_to_a_tenth_and_a_range_by_both_ends() {
        assert_eq!(shown(0.0), "0.0");
        assert_eq!(shown(0.04), "<0.1");
        assert_eq!(shown(0.06), "0.1");
        assert_eq!(shown(1168.24), "1168.2");
        let r = |low: f64, high: f64| shown_range(Range { low, high });
        assert_eq!(r(30.499999, 30.500001), "30.5");
        assert_eq!(r(115.0, 160.0), "115.0–160.0");
    }

    #[test]
    fn a_report_lists_the_lines_without_grams() {
        let md = report("---\ntitle: Test\n---\n@salt{1%tsp} and @garlic{1}(chopped).")
            .markdown("x", "From x.");
        assert!(md.starts_with("# Test\n"), "{md}");
        assert!(
            md.contains("\nIngredient lines: 2, with grams: 0.\n"),
            "{md}"
        );
        assert!(
            md.contains("| nutrient | unit | total | lines it covers |\n"),
            "{md}"
        );
        assert!(!md.contains("per serving"), "no servings, no column: {md}");
        assert!(
            md.ends_with(
                "## Lines without grams\n\n\
                 - salt, 1 tsp: its name is no ingredient's alias.\n\
                 - garlic, 1 (chopped): its amount has no unit, and the food has no USDA portion of one whole item.\n"
            ),
            "{md}"
        );
    }

    #[test]
    fn a_report_says_a_bare_count_counts_whole_items() {
        let md = report("@onion{1} and @onion{1}(medium).").markdown("x", "From x.");
        for how in [
            "| 70.0–150.0 | whole items, by USDA's portions `small`; `medium (2-1/2\" dia)`; `large` |",
            "| 110.0 | whole items, by USDA's portion the note names, `medium (2-1/2\" dia)` |",
        ] {
            assert!(md.contains(how), "{how}\n{md}");
        }
        for (how, said) in [
            (
                How::Whole(texts(&["a"])),
                "whole items, by USDA's portion `a`",
            ),
            (
                How::Whole(texts(&["a", "b"])),
                "whole items, by USDA's portions `a`; `b`",
            ),
            (
                How::WholeByNote(texts(&["a"])),
                "whole items, by USDA's portion the note names, `a`",
            ),
            (
                How::WholeByNote(texts(&["a", "b"])),
                "whole items, by USDA's portions the note names, `a`; `b`",
            ),
        ] {
            assert_eq!(how.said(), said);
        }
    }

    #[test]
    fn a_report_says_a_bare_count_counts_in_its_name_s_unit() {
        let md = report("@garlic cloves{2} and @garlic slices{2}.").markdown("x", "From x.");
        for said in [
            "| 6.0 | `clove`, its name's unit: USDA's portions `clove`; `cloves` |",
            "- garlic slices, 2: its name's unit is `slice`, and the food has no USDA portion in it.\n",
        ] {
            assert!(md.contains(said), "{said}\n{md}");
        }
        let by_note = How::Named("slice".into(), Box::new(How::PortionsByNote(texts(&["a"]))));
        assert_eq!(
            by_note.said(),
            "`slice`, its name's unit: USDA's portion the note names, `a`"
        );
    }

    /// Against the export, a bare count by each alias that ends in a count unit's name counts in
    /// that unit: garlic's two, in cloves of 3 g.
    #[test]
    fn an_alias_s_unit_weighs_a_bare_count_in_the_export() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let export = Export::load(&root).unwrap();
        let got: Vec<String> = analyze("@garlic cloves{2} and @garlic clove{1}.", &export)
            .unwrap()
            .lines
            .iter()
            .map(|l| match &l.grams {
                Ok((g, How::Named(u, _))) => format!("{} {} {u}", l.name, shown_range(*g)),
                other => format!("{} {other:?}", l.name),
            })
            .collect();
        assert_eq!(got, ["garlic cloves 6.0 clove", "garlic clove 3.0 clove"]);
    }

    /// A bare count of each ingredient, by its first alias and in its default form, against the
    /// export: one whole item's grams, or none. A unit marked whole or not, or a new ingredient,
    /// changes this list.
    #[test]
    fn a_bare_count_of_each_ingredient_is_weighed_by_its_whole_items() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let export = Export::load(&root).unwrap();
        let (t, [alias, ingredient]) =
            columns(&export, "ingredient_alias", ["alias", "ingredient"]).unwrap();
        let mut first = BTreeMap::new();
        for r in &t.rows {
            if let (Some(a), Some(i)) = (text(&r[alias]), text(&r[ingredient])) {
                first.entry(i.to_string()).or_insert_with(|| a.to_string());
            }
        }
        let src: String = first.values().map(|a| format!("@{a}{{1}} ")).collect();
        let got: Vec<String> = analyze(&src, &export)
            .unwrap()
            .lines
            .iter()
            .map(|l| {
                let i = l.food.as_ref().map_or("no food", |f| f.ingredient.as_str());
                match &l.grams {
                    Ok((g, _)) => format!("{i} {}", shown_range(*g)),
                    Err(NoGrams::NoWhole) => format!("{i} none"),
                    Err(e) => format!("{i} {e:?}"),
                }
            })
            .collect();
        assert_eq!(
            got,
            [
                "baking-powder none",
                "baking-soda none",
                "black-pepper none",
                "brown-rice none",
                "butter none",
                "carrot 50.0–72.0",
                "chickpeas none",
                "cumin none",
                "eggs 38.0–50.0",
                "flour none",
                "garlic none",
                "lemon 58.0–84.0",
                "lemon-juice none",
                "lentils none",
                "milk none",
                "olive-oil none",
                "onion 70.0–150.0",
                "parmesan none",
                "potato 92.0–369.0",
                "red-lentils none",
                "salt none",
                "soybeans none",
                "spinach none",
                "sugar none",
                "tofu none",
                "tomato 91.0–182.0",
                "vanilla none",
                "vegetable-broth none",
                "water none",
            ]
        );
    }

    #[test]
    fn a_recipe_record_parses_without_warnings() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let names = recipe_names(&root).unwrap();
        assert!(!names.is_empty());
        for name in names {
            let path = format!("{RECIPES_DIR}/{name}.cook");
            let text = fs::read_to_string(root.join(&path)).unwrap();
            assert_eq!(check_record(&text), Ok(()), "{path}");
        }
        let head = "---\ntitle: Soup\nservings: 2\n---\n";
        assert_eq!(check_record(&format!("{head}@a{{1%cup}}.")), Ok(()));
        let refused = |text: &str, why: &str| {
            let err = check_record(text).expect_err(why);
            assert!(err.contains(why), "{err:?} does not say {why:?}");
        };
        refused("---\nservings: 2\n---\n@a{1%cup}.", "no title");
        refused("---\ntitle: Soup\n---\n@a{1%cup}.", "servings is None");
        refused(
            "---\ntitle: Soup\nservings: some\n---\n",
            "Unsupported value for key: 'servings'",
        );
        refused(
            &format!("{head}@a{{1%cup}} and @&a{{1}}."),
            "cooklang warns",
        );
        refused(
            &format!("{head}@a{{1}}(x) and @&a{{1}}(y)."),
            "Note not allowed",
        );
        let dir = std::env::temp_dir().join(format!("vbci-recipes-{}", std::process::id()));
        fs::create_dir_all(dir.join(RECIPES_DIR)).unwrap();
        fs::write(dir.join(RECIPES_DIR).join("soup.txt"), "").unwrap();
        let err = recipe_names(&dir).unwrap_err();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(err, "recipes/soup.txt is not a .cook file");
    }

    #[test]
    fn the_recipe_reports_are_current() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let reports = reports(&root).unwrap();
        for (path, text) in &reports {
            let written = fs::read_to_string(root.join(path)).unwrap_or_default();
            assert!(
                written == *text,
                "{path} is not what crates/core writes: run `cargo run -p balaena-warehouse -- recipes`"
            );
        }
        let mut written: Vec<String> = fs::read_dir(root.join(REPORTS_DIR))
            .unwrap()
            .map(|e| format!("{REPORTS_DIR}/{}", e.unwrap().file_name().to_string_lossy()))
            .collect();
        written.sort();
        let expected: Vec<&String> = reports.iter().map(|(p, _)| p).collect();
        assert_eq!(
            written.iter().collect::<Vec<_>>(),
            expected,
            "a report without its recipe"
        );
    }
}
