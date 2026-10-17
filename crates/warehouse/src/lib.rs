//! VBCI's warehouse, in DuckDB: rebuilt from the records, USDA's pinned zips and Wikidata's pinned
//! revisions on every load, so it can be deleted at any time. `schema.sql` describes its layers: bronze, silver and gold.

pub mod export;
pub mod portions;
pub mod usda;
pub mod wikidata;

use balaena_records::{
    CountUnits, Crops, Diet, IngredientOrganism, IngredientOrigin, Ingredients, Nutrients, Sources,
};
use duckdb::{Connection, params};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs, process};

const SCHEMA: &str = include_str!("../schema.sql");

/// The repo's root, from this crate's place in it.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Where USDA's zips and Wikidata's revisions are cached: `$VBCI_CACHE`, else `$XDG_CACHE_HOME/vbci`, else
/// `$HOME/.cache/vbci`.
pub fn cache_dir() -> Result<PathBuf, String> {
    cache_dir_from(|key| env::var(key).ok().filter(|v| !v.is_empty()))
}

fn cache_dir_from(var: impl Fn(&str) -> Option<String>) -> Result<PathBuf, String> {
    if let Some(dir) = var("VBCI_CACHE") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = var("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(dir).join("vbci"));
    }
    var("HOME")
        .map(|home| PathBuf::from(home).join(".cache").join("vbci"))
        .ok_or_else(|| "no cache: set VBCI_CACHE, XDG_CACHE_HOME or HOME".to_string())
}

/// The bronze table a file in a zip is loaded into: `usda_<name>` for `<name>.csv`, or None for a
/// file that is not CSV. A CSV whose name is not lowercase letters, digits and underscores is
/// refused rather than renamed, so no two files can land in one table.
fn csv_table(member: &str) -> Result<Option<String>, String> {
    let file = member.rsplit('/').next().unwrap_or(member);
    let Some(stem) = file.strip_suffix(".csv") else {
        return Ok(None);
    };
    let plain = !stem.is_empty()
        && stem
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !plain {
        return Err(format!(
            "{member}: a CSV's name must be lowercase letters, digits and underscores"
        ));
    }
    Ok(Some(format!("usda_{stem}")))
}

/// `s` as a SQL string literal.
fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// A directory this process made under the system's temp dir, removed when dropped. Its name
/// carries a count kept by the process as well as the time: on macOS the clock advances by whole
/// microseconds, and two tests that made one in the same microsecond collided ("File exists",
/// 2026-10-11).
struct Scratch(PathBuf);

impl Scratch {
    /// The directory's name, from the process, the time and the process's count.
    fn name(process: u32, nanos: u128, n: u64) -> String {
        format!("balaena-warehouse-{process}-{nanos}-{n}")
    }

    fn new() -> Result<Self, String> {
        static MADE: AtomicU64 = AtomicU64::new(0);
        let n = MADE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = env::temp_dir().join(Self::name(process::id(), nanos, n));
        fs::create_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn sql(e: duckdb::Error) -> String {
    e.to_string()
}

/// Builds the warehouse in memory from the records under `root` and the pinned zips and Wikidata
/// revisions in `cache`. Bronze takes the records as read, each dataset's zip as pinned, every file
/// in each zip with its sha256, every CSV row for row, as text, and each pinned Wikidata revision's
/// content; then silver and gold.
pub fn load(root: &Path, cache: &Path) -> Result<Connection, String> {
    let (sources, sources_text) = Sources::load(root)?;
    let (nutrients, nutrients_text) = Nutrients::load(root)?;
    let diet = Diet::load(root)?;
    let ingredients = Ingredients::load(root)?;
    let crops = Crops::load(root)?;
    let (counts, counts_text) = CountUnits::load(root)?;
    let conn = Connection::open_in_memory().map_err(sql)?;
    conn.execute_batch(SCHEMA).map_err(sql)?;
    let mut records = vec![
        (Sources::PATH.to_string(), sources_text),
        (Nutrients::PATH.to_string(), nutrients_text),
    ];
    records.extend(diet.files.iter().cloned());
    records.push((CountUnits::PATH.to_string(), counts_text));
    records.extend(
        ingredients
            .ingredient
            .iter()
            .map(|i| (i.path.clone(), i.text.clone())),
    );
    records.extend(crops.crop.iter().map(|c| (c.path.clone(), c.text.clone())));
    for (path, text) in &records {
        conn.execute(
            "INSERT INTO bronze.record VALUES (?, ?)",
            params![path, text],
        )
        .map_err(sql)?;
    }
    let scratch = Scratch::new()?;
    let mut tables: BTreeMap<String, Vec<(String, PathBuf)>> = BTreeMap::new();
    for d in &sources.dataset {
        conn.execute(
            "INSERT INTO bronze.usda_zip VALUES (?, ?, ?, ?)",
            params![d.id, d.url, d.sha256, d.bytes as i64],
        )
        .map_err(sql)?;
        for m in usda::extract(cache, d, &scratch.0)? {
            let table = csv_table(&m.name)?;
            conn.execute(
                "INSERT INTO bronze.usda_member VALUES (?, ?, ?, ?, ?)",
                params![d.id, m.name, m.bytes as i64, m.sha256, table],
            )
            .map_err(sql)?;
            if let Some(table) = table {
                tables
                    .entry(table)
                    .or_default()
                    .push((d.id.clone(), m.file));
            }
        }
    }
    for (table, files) in &tables {
        let selects: Vec<String> = files
            .iter()
            .map(|(dataset, file)| {
                format!(
                    "SELECT {} AS dataset, * FROM read_csv({}, header = true, all_varchar = true, strict_mode = true)",
                    literal(dataset),
                    literal(&file.to_string_lossy())
                )
            })
            .collect();
        let stem = table.trim_start_matches("usda_");
        conn.execute_batch(&format!(
            "CREATE TABLE bronze.{table} AS {};\n\
             COMMENT ON TABLE bronze.{table} IS {};\n\
             COMMENT ON COLUMN bronze.{table}.dataset IS 'The dataset''s id in sources.toml: the zip the row came from.';",
            selects.join(" UNION ALL BY NAME "),
            literal(&format!(
                "Every row of {stem}.csv in each dataset's zip, as read: every column text."
            )),
        ))
        .map_err(|e| format!("bronze.{table}: {e}"))?;
    }
    for w in &sources.wikidata {
        let content = wikidata::read(cache, w)?;
        conn.execute(
            "INSERT INTO bronze.wikidata_revision VALUES (?, ?, ?, ?, ?)",
            params![w.item, w.revision as i64, w.sha256, w.bytes as i64, content],
        )
        .map_err(|e| format!("bronze.wikidata_revision {}: {e}", w.item))?;
    }
    load_silver(&conn, &nutrients)?;
    load_diet(&conn, &diet)?;
    load_ingredients(&conn, &ingredients)?;
    load_organisms(&conn, &ingredients, &crops)?;
    load_portions(&conn, &counts)?;
    Ok(conn)
}

/// Loads USDA's portions of silver foods and the count units into silver, refusing a count unit's
/// name cooklang knows; a portion whose amount or gram weight is not above 0 is rejected with its
/// reason. Then reads each kept portion's unit from its text: one row per portion, with no unit
/// where none is read.
fn load_portions(conn: &Connection, counts: &CountUnits) -> Result<(), String> {
    let converter = cooklang::convert::Converter::bundled();
    portions::check_count_units(counts, &converter)?;
    conn.execute_batch(
        "INSERT INTO silver.usda_measure_unit
         SELECT dataset, CAST(id AS INTEGER), name FROM bronze.usda_measure_unit;
         CREATE TEMP TABLE portion_in_scope AS
         SELECT p.dataset, p.id, CAST(p.fdc_id AS INTEGER) AS fdc_id,
                CAST(p.amount AS DOUBLE) AS amount, CAST(p.measure_unit_id AS INTEGER) AS measure_unit_id,
                p.modifier, CAST(p.gram_weight AS DOUBLE) AS gram_weight,
                CASE WHEN m.name = 'undetermined' THEN coalesce(p.modifier, '') ELSE m.name END AS unit_text
         FROM bronze.usda_food_portion p
         LEFT JOIN bronze.usda_measure_unit m ON m.dataset = p.dataset AND m.id = p.measure_unit_id
         WHERE CAST(p.fdc_id AS INTEGER) IN (SELECT fdc_id FROM silver.usda_food);
         INSERT INTO silver.usda_rejected
         SELECT 'usda_food_portion', dataset, id,
                concat_ws('; ', CASE WHEN amount <= 0 THEN 'amount is ' || amount || ', so no grams per unit' END,
                                CASE WHEN gram_weight <= 0 THEN 'gram weight is ' || gram_weight END)
         FROM portion_in_scope WHERE amount <= 0 OR gram_weight <= 0;
         INSERT INTO silver.usda_portion
         SELECT * FROM portion_in_scope WHERE amount > 0 AND gram_weight > 0;
         DROP TABLE portion_in_scope;",
    )
    .map_err(|e| format!("silver.usda_portion: {e}"))?;
    for u in &counts.unit {
        conn.execute(
            "INSERT INTO silver.count_unit VALUES (?, ?, ?)",
            params![u.key, u.is_whole(), CountUnits::PATH],
        )
        .map_err(|e| format!("{} {}: {e}", CountUnits::PATH, u.key))?;
        for name in &u.names {
            conn.execute(
                "INSERT INTO silver.count_unit_name VALUES (?, ?)",
                params![name, u.key],
            )
            .map_err(|e| format!("{} {name:?}: {e}", CountUnits::PATH))?;
        }
    }
    let texts = rows(
        conn,
        "SELECT dataset, id, unit_text FROM silver.usda_portion ORDER BY dataset, id",
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        },
    )?;
    conn.execute_batch("BEGIN").map_err(sql)?;
    let mut insert = conn
        .prepare("INSERT INTO silver.usda_portion_unit VALUES (?, ?, ?, ?, ?, ?)")
        .map_err(sql)?;
    for (dataset, id, text) in &texts {
        let read = portions::portion_unit(text, &converter, counts);
        let phrase = read
            .as_ref()
            .map(|u| u.phrase.clone())
            .unwrap_or_else(|| portions::leading_phrase(text));
        insert
            .execute(params![
                dataset,
                id,
                phrase,
                read.as_ref().map(|u| u.kind),
                read.as_ref().map(|u| u.unit.clone()),
                read.as_ref().and_then(|u| u.per_unit)
            ])
            .map_err(|e| format!("silver.usda_portion_unit {dataset} {id}: {e}"))?;
    }
    conn.execute_batch("COMMIT").map_err(sql)?;
    Ok(())
}

/// Loads the FoodOn ids and common names USDA's data gives foods, and the ingredients with their
/// origins, into silver; then refuses a form whose name is not USDA's. A form linked to a food
/// silver lacks, an alias two ingredients claim, or an ingredient made from one not there, the
/// keys refuse.
fn load_ingredients(conn: &Connection, ingredients: &Ingredients) -> Result<(), String> {
    conn.execute_batch(
        "INSERT INTO silver.usda_food_foodon
         SELECT DISTINCT CAST(fdc_id AS INTEGER), dataset,
                replace(value, 'http://purl.obolibrary.org/obo/', '')
         FROM bronze.usda_food_attribute
         WHERE name = 'FoodOn Ontology ID For FDC Item' AND value <> 'NA'
           AND CAST(fdc_id AS INTEGER) IN (SELECT fdc_id FROM silver.usda_food);
         INSERT INTO silver.usda_food_common_name
         SELECT DISTINCT CAST(fdc_id AS INTEGER), dataset, value
         FROM bronze.usda_food_attribute
         WHERE food_attribute_type_id = '1000' AND trim(coalesce(value, '')) <> ''
           AND CAST(fdc_id AS INTEGER) IN (SELECT fdc_id FROM silver.usda_food);",
    )
    .map_err(|e| format!("silver.usda_food_foodon or usda_food_common_name: {e}"))?;
    for i in &ingredients.ingredient {
        conn.execute(
            "INSERT INTO silver.ingredient VALUES (?, ?, ?, ?)",
            params![i.id, i.name, i.wikidata, i.path],
        )
        .map_err(|e| format!("{}: {e}", i.path))?;
        for alias in &i.aliases {
            conn.execute(
                "INSERT INTO silver.ingredient_alias VALUES (?, ?)",
                params![alias, i.id],
            )
            .map_err(|e| format!("{} alias {alias:?}: {e}", i.path))?;
        }
        for (position, form) in i.forms.iter().enumerate() {
            conn.execute(
                "INSERT INTO silver.ingredient_form VALUES (?, ?, ?, ?, ?)",
                params![
                    i.id,
                    form.state.as_str(),
                    position as i64 + 1,
                    form.fdc_id,
                    form.description
                ],
            )
            .map_err(|e| format!("{} {} {}: {e}", i.path, form.state.as_str(), form.fdc_id))?;
        }
    }
    for i in &ingredients.ingredient {
        let (origin, sources, none) = match &i.origin {
            IngredientOrigin::Harvested => ("harvested", &[][..], None),
            IngredientOrigin::MadeFrom(sources) => ("made_from", &sources[..], None),
            IngredientOrigin::None { why } => ("none", &[][..], Some(why)),
        };
        conn.execute(
            "INSERT INTO silver.ingredient_origin VALUES (?, ?, ?)",
            params![i.id, origin, none],
        )
        .map_err(|e| format!("{} origin: {e}", i.path))?;
        for (position, source) in sources.iter().enumerate() {
            conn.execute(
                "INSERT INTO silver.ingredient_made_from VALUES (?, ?, ?)",
                params![i.id, position as i64 + 1, source],
            )
            .map_err(|e| format!("{} made_from {source}: {e}", i.path))?;
        }
    }
    refuse_any(
        conn,
        "SELECT i.record || ': food ' || f.fdc_id || ' is ' || u.description
                || ' in USDA''s data, not ' || f.description
         FROM silver.ingredient_form f
         JOIN silver.ingredient i USING (ingredient)
         JOIN silver.usda_food u USING (fdc_id)
         WHERE f.description <> u.description
         ORDER BY 1",
    )
}

/// Loads the organism USDA's data gives each Foundation food, the statements `wikidata::PROPERTIES`
/// names in each pinned Wikidata item, each ingredient's organism and each crop into silver. Then
/// refuses an ingredient's organism its source does not give; a crop whose item does not give its
/// taxon and name, whose ingredient is not harvested, or whose organism is neither its
/// ingredient's nor one step below it; and a pinned item nothing reads. A food, item or ingredient form named but not there, the keys and the
/// Wikidata reading refuse.
fn load_organisms(
    conn: &Connection,
    ingredients: &Ingredients,
    crops: &Crops,
) -> Result<(), String> {
    conn.execute_batch(
        "INSERT INTO silver.usda_food_organism
         SELECT DISTINCT CAST(fdc_id AS INTEGER), dataset,
                replace(value, 'http://purl.obolibrary.org/obo/', '')
         FROM bronze.usda_food_attribute
         WHERE name = 'NCBI Taxon' AND value <> 'NA'
           AND CAST(fdc_id AS INTEGER) IN (SELECT fdc_id FROM silver.usda_food);",
    )
    .map_err(|e| format!("silver.usda_food_organism: {e}"))?;
    let revisions = rows(
        conn,
        "SELECT item, content FROM bronze.wikidata_revision ORDER BY item",
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )?;
    let mut pinned = BTreeMap::new();
    for (item, content) in revisions {
        let statements = wikidata::statements(&item, &content)?;
        let mut positions: BTreeMap<&str, i64> = BTreeMap::new();
        for st in &statements {
            let position = positions.entry(st.property).or_default();
            *position += 1;
            conn.execute(
                "INSERT INTO silver.wikidata_statement VALUES (?, ?, ?, ?, ?)",
                params![item, st.property, *position, st.value, st.rank],
            )
            .map_err(|e| format!("silver.wikidata_statement {item}: {e}"))?;
        }
        pinned.insert(item, statements);
    }
    let mut read = BTreeSet::new();
    for i in &ingredients.ingredient {
        let (mut fdc_id, mut description, mut item, mut taxon_item, mut none) =
            (None, None, None, None, None);
        match &i.organism {
            IngredientOrganism::Usda {
                fdc_id: f,
                description: d,
                ..
            } => (fdc_id, description) = (Some(*f), Some(d)),
            IngredientOrganism::Wikidata { taxon, item: q } => {
                let (got, from) = wikidata::taxon(q, &pinned)
                    .map_err(|e| format!("{}: organism: {e}", i.path))?;
                if got != *taxon {
                    return Err(format!(
                        "{}: organism: Wikidata gives {got}, not {taxon}",
                        i.path
                    ));
                }
                read.insert(q.clone());
                read.insert(from.clone());
                (item, taxon_item) = (Some(q), Some(from));
            }
            IngredientOrganism::None { why } => none = Some(why),
        }
        conn.execute(
            "INSERT INTO silver.ingredient_organism VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                i.id,
                i.organism.taxon(),
                fdc_id,
                description,
                item,
                taxon_item,
                none
            ],
        )
        .map_err(|e| format!("{} organism: {e}", i.path))?;
    }
    refuse_any(
        conn,
        "SELECT i.record || ': organism: food ' || o.fdc_id || ' is ' || u.description
                || ' in USDA''s data, not ' || o.description
         FROM silver.ingredient_organism o
         JOIN silver.ingredient i USING (ingredient)
         JOIN silver.usda_food u USING (fdc_id)
         WHERE o.description <> u.description
         ORDER BY 1",
    )?;
    refuse_any(
        conn,
        "SELECT i.record || ': organism: food ' || o.fdc_id || ' records no NCBI Taxon in USDA''s data'
         FROM silver.ingredient_organism o
         JOIN silver.ingredient i USING (ingredient)
         WHERE o.fdc_id NOT IN (SELECT fdc_id FROM silver.usda_food_organism)
         ORDER BY 1",
    )?;
    refuse_any(
        conn,
        "SELECT i.record || ': organism: food ' || o.fdc_id || ' records ' || f.taxon
                || ' in USDA''s data, not ' || o.taxon
         FROM silver.ingredient_organism o
         JOIN silver.ingredient i USING (ingredient)
         JOIN silver.usda_food_organism f USING (fdc_id)
         WHERE o.taxon <> f.taxon
         ORDER BY 1",
    )?;
    let organism: BTreeMap<&str, Option<&str>> = ingredients
        .ingredient
        .iter()
        .map(|i| (i.id.as_str(), i.organism.taxon()))
        .collect();
    let harvested: BTreeSet<&str> = ingredients
        .ingredient
        .iter()
        .filter(|i| i.origin == IngredientOrigin::Harvested)
        .map(|i| i.id.as_str())
        .collect();
    for c in &crops.crop {
        let at = |e: String| format!("{}: {e}", c.path);
        let (taxon, name) = wikidata::own_taxon(&c.wikidata, &pinned).map_err(at)?;
        if taxon != c.taxon {
            return Err(at(format!("Wikidata gives {taxon}, not {}", c.taxon)));
        }
        if name != c.scientific_name {
            return Err(at(format!(
                "Wikidata item {} names {taxon} {name}, not {}",
                c.wikidata, c.scientific_name
            )));
        }
        read.insert(c.wikidata.clone());
        let ingredient = organism
            .get(c.ingredient.as_str())
            .ok_or_else(|| {
                at(format!(
                    "ingredient {} is not in ingredients/",
                    c.ingredient
                ))
            })?
            .ok_or_else(|| {
                at(format!(
                    "ingredient {} has no organism, so no crop yields it",
                    c.ingredient
                ))
            })?;
        if !harvested.contains(c.ingredient.as_str()) {
            return Err(at(format!(
                "ingredient {} is not harvested, so no crop yields it: its [origin] is not harvested = true",
                c.ingredient
            )));
        }
        let parent_item = if taxon == ingredient {
            None
        } else {
            let (parent, parent_taxon) =
                wikidata::parent_taxon(&c.wikidata, &pinned).map_err(at)?;
            if parent_taxon != ingredient {
                return Err(at(format!(
                    "{taxon} is neither {}'s organism, {ingredient}, nor one step below it: \
                     Wikidata item {}'s {} is {parent}, {parent_taxon}",
                    c.ingredient,
                    c.wikidata,
                    wikidata::PARENT_TAXON
                )));
            }
            read.insert(parent.clone());
            Some(parent)
        };
        conn.execute(
            "INSERT INTO silver.crop VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                c.id,
                c.name,
                c.wikidata,
                c.taxon,
                c.scientific_name,
                c.ingredient,
                c.state.as_str(),
                parent_item,
                c.path
            ],
        )
        .map_err(|e| at(e.to_string()))?;
    }
    if let Some(item) = pinned.keys().find(|q| !read.contains(*q)) {
        return Err(format!(
            "sources.toml: Wikidata item {item} is pinned, but no ingredient's or crop's organism reads it"
        ));
    }
    Ok(())
}

/// The first row of `query`, which names what is wrong, as an error; Ok when there is none.
fn refuse_any(conn: &Connection, query: &str) -> Result<(), String> {
    match rows(conn, query, |r| r.get::<_, String>(0))?
        .into_iter()
        .next()
    {
        Some(wrong) => Err(wrong),
        None => Ok(()),
    }
}

/// Loads each Foundation food's organism group and the four diet records into silver, then refuses
/// what the records' comments rule out: a category with no class, a class for a category no
/// dataset has, an organism the records lack, a rule that matches no food, a food two rules match,
/// and an exception whose name is not USDA's.
fn load_diet(conn: &Connection, diet: &Diet) -> Result<(), String> {
    conn.execute_batch(
        "INSERT INTO silver.usda_food_taxon
         SELECT DISTINCT CAST(fdc_id AS INTEGER), dataset,
                replace(value, 'http://purl.obolibrary.org/obo/', '')
         FROM bronze.usda_food_attribute
         WHERE name = 'NCBI Taxon Parent' AND value <> 'NA'
           AND CAST(fdc_id AS INTEGER) IN (SELECT fdc_id FROM silver.usda_food);",
    )
    .map_err(|e| format!("silver.usda_food_taxon: {e}"))?;
    let [categories, taxa, rules, exceptions] = Diet::FILES.map(|f| format!("{}/{f}", Diet::DIR));
    let answer = |a: Option<balaena_records::Answer>| a.map(|a| a.as_str());
    for c in &diet.categories {
        conn.execute(
            "INSERT INTO silver.diet_category VALUES (?, ?, ?)",
            params![c.description, c.class.as_str(), categories],
        )
        .map_err(|e| format!("{categories} {:?}: {e}", c.description))?;
    }
    for t in &diet.taxa {
        conn.execute(
            "INSERT INTO silver.diet_taxon VALUES (?, ?, ?, ?)",
            params![t.id, t.name, t.group.as_str(), taxa],
        )
        .map_err(|e| format!("{taxa} {}: {e}", t.id))?;
    }
    for r in &diet.rules {
        conn.execute(
            "INSERT INTO silver.diet_rule VALUES (?, ?, ?, ?, ?, ?)",
            params![
                r.category,
                r.starts_with,
                answer(r.vegan),
                answer(r.vegetarian),
                r.why,
                rules
            ],
        )
        .map_err(|e| format!("{rules} {} / {:?}: {e}", r.category, r.starts_with))?;
    }
    for x in &diet.exceptions {
        conn.execute(
            "INSERT INTO silver.diet_exception VALUES (?, ?, ?, ?, ?, ?)",
            params![
                x.fdc_id,
                x.description,
                answer(x.vegan),
                answer(x.vegetarian),
                x.why,
                exceptions
            ],
        )
        .map_err(|e| format!("{exceptions} {}: {e}", x.fdc_id))?;
    }
    refuse_any(
        conn,
        &format!(
            "SELECT '{categories} has no class for category ' || c.description
             FROM silver.usda_category c
             WHERE c.description NOT IN (SELECT description FROM silver.diet_category)
             ORDER BY 1"
        ),
    )?;
    refuse_any(
        conn,
        &format!(
            "SELECT '{categories}: category ' || description || ' is in no dataset'
             FROM silver.diet_category
             WHERE description NOT IN (SELECT description FROM silver.usda_category)
             ORDER BY 1"
        ),
    )?;
    refuse_any(
        conn,
        &format!(
            "SELECT '{taxa} lacks ' || taxon || ', which food ' || fdc_id || ' records'
             FROM silver.usda_food_taxon
             WHERE taxon NOT IN (SELECT taxon FROM silver.diet_taxon)
             ORDER BY 1"
        ),
    )?;
    let matches = "SELECT r.category, r.starts_with, f.fdc_id
                   FROM silver.diet_rule r
                   JOIN silver.usda_category c ON c.description = r.category
                   JOIN silver.usda_food f ON f.dataset = c.dataset AND f.category_id = c.id
                   WHERE starts_with(f.description, r.starts_with)";
    refuse_any(
        conn,
        &format!(
            "SELECT '{rules}: no food in ' || r.category || ' starts with ' || r.starts_with
             FROM silver.diet_rule r
             WHERE NOT EXISTS (SELECT 1 FROM ({matches}) m
                               WHERE m.category = r.category AND m.starts_with = r.starts_with)
             ORDER BY 1"
        ),
    )?;
    refuse_any(
        conn,
        &format!(
            "SELECT '{rules}: food ' || fdc_id || ' matches two rules: '
                    || string_agg(starts_with, ' and ' ORDER BY starts_with)
             FROM ({matches}) GROUP BY fdc_id HAVING count(*) > 1 ORDER BY 1"
        ),
    )?;
    refuse_any(
        conn,
        &format!(
            "SELECT '{exceptions}: food ' || x.fdc_id || ' is ' || f.description
                    || ' in USDA''s data, not ' || x.description
             FROM silver.diet_exception x JOIN silver.usda_food f USING (fdc_id)
             WHERE x.description <> f.description
             ORDER BY 1"
        ),
    )?;
    Ok(())
}

/// Types bronze's USDA rows into silver, keeping each nutrient row of a silver food or rejecting it
/// with its reason (`SILVER` says which), and loads `nutrients.toml`. Then refuses a nutrient code that some dataset lacks, or has
/// with another unit: a code is matched exactly, so a misspelt or rounded code would silently find
/// nothing, or another nutrient.
fn load_silver(conn: &Connection, nutrients: &Nutrients) -> Result<(), String> {
    conn.execute_batch(SILVER)
        .map_err(|e| format!("silver: {e}"))?;
    for (position, n) in nutrients.nutrient.iter().enumerate() {
        conn.execute(
            "INSERT INTO silver.nutrient VALUES (?, ?, ?, ?, ?)",
            params![n.key, position as i64 + 1, n.name, n.unit, Nutrients::PATH],
        )
        .map_err(|e| format!("silver.nutrient {}: {e}", n.key))?;
        for (priority, code) in n.codes.iter().enumerate() {
            conn.execute(
                "INSERT INTO silver.nutrient_code VALUES (?, ?, ?)",
                params![n.key, priority as i64 + 1, code],
            )
            .map_err(|e| format!("silver.nutrient_code {} {code}: {e}", n.key))?;
        }
    }
    let mut stmt = conn
        .prepare(
            "SELECT n.nutrient, c.code, z.dataset, n.unit, u.unit
             FROM silver.nutrient n
             JOIN silver.nutrient_code c USING (nutrient)
             CROSS JOIN bronze.usda_zip z
             LEFT JOIN silver.usda_nutrient u ON u.dataset = z.dataset AND u.nbr = c.code
             WHERE u.unit IS DISTINCT FROM n.unit
             ORDER BY n.position, c.priority, z.dataset",
        )
        .map_err(sql)?;
    let wrong: Vec<(String, String, String, String, Option<String>)> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .map_err(sql)?
        .collect::<Result<_, _>>()
        .map_err(sql)?;
    if let Some((nutrient, code, dataset, want, got)) = wrong.into_iter().next() {
        return Err(match got {
            None => format!(
                "{}: {nutrient}'s code {code} is not in {dataset}'s nutrient.csv",
                Nutrients::PATH
            ),
            Some(got) => format!(
                "{}: {nutrient}'s code {code} has unit {got} in {dataset}, not {want}",
                Nutrients::PATH
            ),
        });
    }
    Ok(())
}

/// Silver's USDA rows, typed from bronze. A food_nutrient row of a silver food is rejected, with
/// its reason, when it has no amount, names a nutrient its dataset's nutrient.csv lacks, repeats an
/// earlier row for the same food and nutrient with the same amount (the earliest row by id is
/// kept), or disagrees with another row for the same food and nutrient (every such row is
/// rejected, so the food has no value rather than a guess). Every other row is typed, and a value
/// that does not cast stops the load.
const SILVER: &str = "
INSERT INTO silver.usda_category
SELECT dataset, CAST(id AS INTEGER), description FROM bronze.usda_food_category;

INSERT INTO silver.usda_food
SELECT CAST(fdc_id AS INTEGER), dataset, data_type, description, CAST(food_category_id AS INTEGER)
FROM bronze.usda_food
WHERE data_type IN ('sr_legacy_food', 'foundation_food');

INSERT INTO silver.usda_nutrient
SELECT dataset, CAST(id AS INTEGER), name, unit_name, nullif(nutrient_nbr, '')
FROM bronze.usda_nutrient;

CREATE TEMP TABLE in_scope AS
SELECT b.dataset, b.id, CAST(b.fdc_id AS INTEGER) AS fdc_id,
       CAST(b.nutrient_id AS INTEGER) AS nutrient_id, b.amount,
       coalesce(b.amount, '') = '' AS no_amount,
       NOT EXISTS (SELECT 1 FROM silver.usda_nutrient n
                   WHERE n.dataset = b.dataset AND n.id = CAST(b.nutrient_id AS INTEGER)) AS no_nutrient
FROM bronze.usda_food_nutrient b
WHERE CAST(b.fdc_id AS INTEGER) IN (SELECT fdc_id FROM silver.usda_food);

CREATE TEMP TABLE typable AS
SELECT dataset, id, fdc_id, nutrient_id, CAST(amount AS DOUBLE) AS amount,
       min(CAST(amount AS DOUBLE)) OVER pair <> max(CAST(amount AS DOUBLE)) OVER pair AS conflicting,
       row_number() OVER (pair ORDER BY CAST(id AS BIGINT)) AS copy,
       first_value(id) OVER (pair ORDER BY CAST(id AS BIGINT)) AS first_id
FROM in_scope
WHERE NOT (no_amount OR no_nutrient)
WINDOW pair AS (PARTITION BY fdc_id, nutrient_id);

INSERT INTO silver.usda_rejected
SELECT 'usda_food_nutrient', dataset, id,
       concat_ws('; ', CASE WHEN no_amount THEN 'no amount' END,
                       CASE WHEN no_nutrient THEN 'nutrient ' || nutrient_id || ' is not in its dataset''s nutrient.csv' END)
FROM in_scope
WHERE no_amount OR no_nutrient
UNION ALL
SELECT 'usda_food_nutrient', dataset, id,
       'another row gives food ' || fdc_id || ' a different amount of nutrient ' || nutrient_id
FROM typable
WHERE conflicting
UNION ALL
SELECT 'usda_food_nutrient', dataset, id,
       'repeats row ' || first_id || ': same food, nutrient and amount'
FROM typable
WHERE NOT conflicting AND copy > 1;

INSERT INTO silver.usda_food_nutrient
SELECT fdc_id, dataset, nutrient_id, amount, id
FROM typable
WHERE NOT conflicting AND copy = 1;

DROP TABLE typable;
DROP TABLE in_scope;
";

/// Where `coverage` writes its report, relative to the repo root.
pub const COVERAGE_PATH: &str = "docs/coverage.md";

/// Where `catalogue` writes the data catalogue, relative to the repo root.
pub const CATALOGUE_PATH: &str = "docs/data-catalogue.md";

fn rows<T>(
    conn: &Connection,
    query: &str,
    f: impl FnMut(&duckdb::Row<'_>) -> duckdb::Result<T>,
) -> Result<Vec<T>, String> {
    let mut stmt = conn.prepare(query).map_err(|e| format!("{query}: {e}"))?;
    stmt.query_map([], f)
        .map_err(|e| format!("{query}: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{query}: {e}"))
}

/// `docs/coverage.md`, as the warehouse writes it: for each dataset, how many of its foods have a
/// value for each nutrient VBCI reports, and which codes the values came from; then the rows silver
/// rejected.
pub fn coverage_report(conn: &Connection, sources: &Sources) -> Result<String, String> {
    let mut out = String::from(
        "# Nutrient coverage\n\n\
         Generated by `cargo run -p balaena-warehouse -- coverage` from the datasets `sources.toml`\n\
         pins and the nutrients `nutrients.toml` lists; the test `the_coverage_report_is_current`\n\
         fails when this file differs from what that would write. Change the records, not this\n\
         file.\n\n\
         For each dataset: how many of its foods have a value for each nutrient, per 100 g\n\
         (`gold.nutrient_coverage`), and how many took it from each of the nutrient's USDA codes,\n\
         first choice first (`gold.nutrient_source`). Every food counts, meat included.\n",
    );
    for d in &sources.dataset {
        let coverage = rows(
            conn,
            &format!(
                "SELECT nutrient, unit, foods, with_value FROM gold.nutrient_coverage \
                 WHERE dataset = {} ORDER BY position",
                literal(&d.id)
            ),
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )?;
        let foods = coverage.first().map(|c| c.2).unwrap_or_default();
        out.push_str(&format!(
            "\n## {} {} (`{}`): {foods} foods\n\n\
             | nutrient | unit | foods with a value | share | from codes |\n|---|---|---|---|---|\n",
            d.name, d.release, d.id
        ));
        for (nutrient, unit, foods, with_value) in coverage {
            let sources = rows(
                conn,
                &format!(
                    "SELECT c.code, coalesce(s.foods, 0) FROM silver.nutrient_code c \
                     LEFT JOIN gold.nutrient_source s ON s.nutrient = c.nutrient \
                     AND s.code = c.code AND s.dataset = {} \
                     WHERE c.nutrient = {} ORDER BY c.priority",
                    literal(&d.id),
                    literal(&nutrient)
                ),
                |r| {
                    Ok(format!(
                        "{}: {}",
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?
                    ))
                },
            )?;
            let share = if foods == 0 {
                0.0
            } else {
                100.0 * with_value as f64 / foods as f64
            };
            let from = sources.join(", ");
            out.push_str(&format!(
                "| {nutrient} | {unit} | {with_value} | {share:.1}% | {from} |\n"
            ));
        }
    }
    let rejected = rows(
        conn,
        "SELECT dataset, source, reason, count(*) FROM silver.usda_rejected \
         GROUP BY ALL ORDER BY dataset, source, reason",
        |r| {
            Ok(format!(
                "| {} | {} | {} | {} |",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?
            ))
        },
    )?;
    out.push_str(
        "\n## Rows silver rejected\n\n\
         Rows of silver foods that silver did not keep (`silver.usda_rejected`).\n\n",
    );
    if rejected.is_empty() {
        out.push_str("None.\n");
    } else {
        out.push_str("| dataset | bronze table | reason | rows |\n|---|---|---|---|\n");
        for line in rejected {
            out.push_str(&line);
            out.push('\n');
        }
    }
    Ok(out)
}

/// Where `diet` writes its report, relative to the repo root.
pub const DIET_PATH: &str = "docs/diet.md";

/// `docs/diet.md`, as the warehouse writes it: for each dataset, how many foods take each answer for
/// each diet, and what decided them; then how many foods each naming rule decides.
pub fn diet_report(conn: &Connection, sources: &Sources) -> Result<String, String> {
    let mut out = String::from(
        "# Diet status\n\n\
         Generated by `cargo run -p balaena-warehouse -- diet` from the datasets `sources.toml` pins\n\
         and the records in `diet/`; the test `the_diet_report_is_current` fails when this file\n\
         differs from what that would write. Change the records, not this file.\n\n\
         Each food's answer for each diet, yes, no or unknown (`gold.food_diet`), and what decided\n\
         it, the first that applies: an exception for the food (`diet/exceptions.toml`), a naming\n\
         rule within its category (`diet/rules.toml`), the animal organism a Foundation food\n\
         records (`diet/taxa.toml`), or its category's class (`diet/categories.toml`). Vegan is The\n\
         Vegan Society's definition and vegetarian the Vegetarian Society's criteria\n\
         (`docs/architecture.md`, \"Decided\"). An answer that came from a category holds for a\n\
         food only as far as its USDA name shows what is in it.\n",
    );
    let count = |query: String| -> Result<i64, String> {
        conn.query_row(&query, [], |r| r.get(0))
            .map_err(|e| format!("{query}: {e}"))
    };
    for d in &sources.dataset {
        let ds = literal(&d.id);
        let foods = count(format!(
            "SELECT count(*) FROM gold.food_diet WHERE dataset = {ds}"
        ))?;
        out.push_str(&format!(
            "\n## {} {} (`{}`): {foods} foods\n\n| answer | vegan | vegetarian |\n|---|---|---|\n",
            d.name, d.release, d.id
        ));
        for answer in ["yes", "no", "unknown"] {
            let a = literal(answer);
            let vegan = count(format!(
                "SELECT count(*) FROM gold.food_diet WHERE dataset = {ds} AND vegan = {a}"
            ))?;
            let vegetarian = count(format!(
                "SELECT count(*) FROM gold.food_diet WHERE dataset = {ds} AND vegetarian = {a}"
            ))?;
            out.push_str(&format!("| {answer} | {vegan} | {vegetarian} |\n"));
        }
        out.push_str("\n| decided by | vegan | vegetarian |\n|---|---|---|\n");
        for (label, pattern) in [
            ("an exception", "exception"),
            ("a naming rule", "rule: %"),
            ("an animal organism", "organism: %"),
            ("the category", "category: %"),
        ] {
            let p = literal(pattern);
            let vegan = count(format!(
                "SELECT count(*) FROM gold.food_diet WHERE dataset = {ds} AND vegan_basis LIKE {p}"
            ))?;
            let vegetarian = count(format!(
                "SELECT count(*) FROM gold.food_diet WHERE dataset = {ds} AND vegetarian_basis LIKE {p}"
            ))?;
            out.push_str(&format!("| {label} | {vegan} | {vegetarian} |\n"));
        }
    }
    out.push_str(
        "\n## Naming rules\n\n\
         How many foods, in all datasets, each rule matches (`diet/rules.toml`); an exception\n\
         for a matched food still overrides the rule.\n\n\
         | category | name starts with | vegan | vegetarian | foods |\n|---|---|---|---|---|\n",
    );
    let lines = rows(
        conn,
        "SELECT r.category, r.starts_with, coalesce(r.vegan, '-'), coalesce(r.vegetarian, '-'),
                (SELECT count(*) FROM gold.food_diet f
                 WHERE f.category = r.category AND starts_with(f.description, r.starts_with))
         FROM silver.diet_rule r ORDER BY r.category, r.starts_with",
        |r| {
            Ok(format!(
                "| {} | {} | {} | {} | {} |",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?
            ))
        },
    )?;
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    let exceptions = count("SELECT count(*) FROM silver.diet_exception".to_string())?;
    out.push_str(&format!(
        "\n{exceptions} foods have an exception, each with its reason, in `diet/exceptions.toml`.\n"
    ));
    Ok(out)
}

/// Where `ingredients` writes its report, relative to the repo root.
pub const INGREDIENTS_PATH: &str = "docs/ingredients.md";

/// `docs/ingredients.md`, as the warehouse writes it: each ingredient with its aliases, its
/// organism, what it is made from and its forms, and the alias candidates USDA's data gives its
/// foods.
pub fn ingredients_report(conn: &Connection) -> Result<String, String> {
    let count = |query: &str| -> Result<i64, String> {
        conn.query_row(query, [], |r| r.get(0))
            .map_err(|e| format!("{query}: {e}"))
    };
    let (ingredients, forms, candidates, nutrients) = (
        count("SELECT count(*) FROM silver.ingredient")?,
        count("SELECT count(*) FROM silver.ingredient_form")?,
        count("SELECT count(*) FROM gold.alias_candidate")?,
        count("SELECT count(*) FROM silver.nutrient")?,
    );
    let (from_usda, from_wikidata, no_organism) = (
        count("SELECT count(*) FROM gold.ingredient_organism WHERE fdc_id IS NOT NULL")?,
        count("SELECT count(*) FROM gold.ingredient_organism WHERE wikidata IS NOT NULL")?,
        count("SELECT count(*) FROM gold.ingredient_organism WHERE none IS NOT NULL")?,
    );
    let (harvested, made, made_of_none) = (
        count("SELECT count(*) FROM gold.ingredient_origin WHERE origin = 'harvested'")?,
        count("SELECT count(*) FROM gold.ingredient_origin WHERE origin = 'made_from'")?,
        count("SELECT count(*) FROM gold.ingredient_origin WHERE origin = 'none'")?,
    );
    let mut out = format!(
        "# Ingredients\n\n\
         Generated by `cargo run -p balaena-warehouse -- ingredients` from `ingredients/` and the\n\
         datasets `sources.toml` pins; the test `the_ingredients_report_is_current` fails when this\n\
         file differs from what that would write. Change the records, not this file.\n\n\
         {ingredients} ingredients, with {forms} forms. Each form is a state linked to one USDA food\n\
         (`gold.ingredient_form`); the first is the default, used when a recipe does not say. \"Of\n\
         {nutrients}\" is how many of the nutrients `nutrients.toml` lists the food has a value for. FoodOn ids\n\
         come from USDA's data, which gives them for Foundation foods only. {candidates} alias\n\
         candidates (`gold.alias_candidate`) are names USDA's data gives a linked food that no\n\
         ingredient lists yet: unknown until a person reviews one into an ingredient's aliases. An\n\
         alias of two or more words whose last word is a count unit's name\n\
         (`gold.ingredient_alias_unit`) is shown with that unit: a recipe line that uses it with no\n\
         unit, such as `@garlic cloves{{2}}`, counts in it.\n\n\
         Each ingredient's organism (`gold.ingredient_organism`) is an NCBI Taxonomy id read from one\n\
         place: the \"NCBI Taxon\" USDA's data gives a Foundation food, for {from_usda} ingredients; a\n\
         Wikidata item `sources.toml` pins, for {from_wikidata}; or none, saying why, for {no_organism}.\n\n\
         What each ingredient is made from (`gold.ingredient_origin`): nothing, as it is harvested, for\n\
         {harvested}; other ingredients (`gold.ingredient_made_from`), for {made}; or something that\n\
         is no ingredient, saying why, for {made_of_none}. A crop yields only a harvested ingredient.\n"
    );
    let ids = rows(
        conn,
        "SELECT ingredient, name FROM silver.ingredient ORDER BY ingredient",
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )?;
    for (id, name) in ids {
        let i = literal(&id);
        let aliases = rows(
            conn,
            &format!(
                "SELECT a.alias || coalesce(' (counts in `' || u.unit || '`)', '')
                 FROM silver.ingredient_alias a LEFT JOIN gold.ingredient_alias_unit u USING (alias)
                 WHERE a.ingredient = {i} ORDER BY a.alias"
            ),
            |r| r.get::<_, String>(0),
        )?;
        let organism = rows(
            conn,
            &format!(
                "SELECT taxon, fdc_id, description, wikidata, taxon_item, none
                 FROM silver.ingredient_organism WHERE ingredient = {i}"
            ),
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            },
        )?;
        let organism = match organism.as_slice() {
            [(Some(taxon), Some(fdc_id), Some(description), None, None, None)] => {
                format!("`{taxon}`, from Foundation food {fdc_id}, {description}.")
            }
            [(Some(taxon), None, None, Some(item), Some(from), None)] if item == from => {
                format!("`{taxon}`, from Wikidata item {item}.")
            }
            [(Some(taxon), None, None, Some(item), Some(from), None)] => format!(
                "`{taxon}`, from Wikidata item {item} through its natural product of taxon, {from}."
            ),
            [(None, None, None, None, None, Some(why))] => format!("none. {why}"),
            other => return Err(format!("{id}: organism {other:?}")),
        };
        let (origin, none) = conn
            .query_row(
                &format!("SELECT origin, none FROM gold.ingredient_origin WHERE ingredient = {i}"),
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .map_err(|e| format!("{id}: origin: {e}"))?;
        let made_from = rows(
            conn,
            &format!(
                "SELECT i.name || ' (`' || m.made_from || '`)' FROM gold.ingredient_made_from m
                 JOIN gold.ingredient i ON i.ingredient = m.made_from
                 WHERE m.ingredient = {i} ORDER BY m.position"
            ),
            |r| r.get::<_, String>(0),
        )?;
        let origin = match (origin.as_str(), none) {
            ("harvested", None) if made_from.is_empty() => "harvested.".to_string(),
            ("made_from", None) if !made_from.is_empty() => {
                format!("made from {}.", made_from.join(", "))
            }
            ("none", Some(why)) if made_from.is_empty() => format!("none. {why}"),
            (other, why) => return Err(format!("{id}: origin {other} {why:?} {made_from:?}")),
        };
        out.push_str(&format!(
            "\n## {name} (`{id}`)\n\nAliases: {}.\n\nOrganism: {organism}\n\nOrigin: {origin}\n\n\
             | state | default | fdc_id | dataset | USDA's name | of {nutrients} | FoodOn |\n|---|---|---|---|---|---|---|\n",
            aliases.join(", ")
        ));
        let forms = rows(
            conn,
            &format!(
                "SELECT f.state, f.is_default, f.fdc_id, f.dataset, f.description, f.nutrients, coalesce(f.foodon, '-')
                 FROM gold.ingredient_form f JOIN silver.ingredient_form s USING (ingredient, state)
                 WHERE f.ingredient = {i} ORDER BY s.position"
            ),
            |r| {
                Ok(format!(
                    "| {} | {} | {} | {} | {} | {} | {} |",
                    r.get::<_, String>(0)?,
                    if r.get::<_, bool>(1)? { "yes" } else { "" },
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, String>(6)?
                ))
            },
        )?;
        for line in forms {
            out.push_str(&line);
            out.push('\n');
        }
        let found = rows(
            conn,
            &format!(
                "SELECT candidate || ' (' || source || ', ' || fdc_id || ')' FROM gold.alias_candidate
                 WHERE ingredient = {i} ORDER BY candidate, source, fdc_id"
            ),
            |r| r.get::<_, String>(0),
        )?;
        if !found.is_empty() {
            out.push_str(&format!("\nAlias candidates: {}.\n", found.join("; ")));
        }
    }
    Ok(out)
}

/// Where `crops` writes its report, relative to the repo root.
pub const CROPS_PATH: &str = "docs/crops.md";

/// `docs/crops.md`, as the warehouse writes it: each crop with the ingredient it yields, its
/// organism, and how that organism stands to its ingredient's.
pub fn crops_report(conn: &Connection) -> Result<String, String> {
    let count = |query: &str| -> Result<i64, String> {
        conn.query_row(query, [], |r| r.get(0))
            .map_err(|e| format!("{query}: {e}"))
    };
    let (crops, ingredients, below) = (
        count("SELECT count(*) FROM gold.crop")?,
        count("SELECT count(DISTINCT ingredient) FROM gold.crop")?,
        count("SELECT count(*) FROM gold.crop WHERE steps_below = 1")?,
    );
    let mut out = format!(
        "# Crops\n\n\
         Generated by `cargo run -p balaena-warehouse -- crops` from `crops/`, `ingredients/` and the\n\
         Wikidata items `sources.toml` pins; the test `the_crops_report_is_current` fails when this\n\
         file differs from what that would write. Change the records, not this file.\n\n\
         {crops} crops yield {ingredients} ingredients (`gold.crop`). A crop names the ingredient it\n\
         yields, one whose origin is harvested (`gold.ingredient_origin`), and the state it is\n\
         harvested in, raw or dry; its organism is the NCBI Taxonomy id and scientific name its\n\
         Wikidata item gives. That organism is its ingredient's, or, for {below}, one step below\n\
         it by the item's parent taxon.\n\n\
         | crop | yields | organism | scientific name | Wikidata | to the ingredient's organism |\n\
         |---|---|---|---|---|---|\n"
    );
    let lines = rows(
        conn,
        "SELECT c.name, c.crop, c.ingredient, c.state, c.taxon, c.scientific_name, c.wikidata,
                c.parent_item, o.taxon
         FROM gold.crop c JOIN gold.ingredient_organism o USING (ingredient)
         ORDER BY c.crop",
        |r| {
            let relation = match r.get::<_, Option<String>>(7)? {
                None => "the same".to_string(),
                Some(parent) => format!(
                    "one step below `{}`, through {parent}",
                    r.get::<_, String>(8)?
                ),
            };
            Ok(format!(
                "| {} (`{}`) | {}, {} | `{}` | {} | {} | {relation} |",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?
            ))
        },
    )?;
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

/// Where `portions` writes its report, relative to the repo root.
pub const PORTIONS_PATH: &str = "docs/portions.md";

/// `docs/portions.md`, as the warehouse writes it: how many portions' units are read as a mass,
/// volume or count unit, every leading phrase no unit reads, and the mass portions whose gram
/// weight disagrees with their unit.
pub fn portions_report(conn: &Connection, sources: &Sources) -> Result<String, String> {
    let count = |query: String| -> Result<i64, String> {
        conn.query_row(&query, [], |r| r.get(0))
            .map_err(|e| format!("{query}: {e}"))
    };
    let mut out = String::from(
        "# Portions\n\n\
         Generated by `cargo run -p balaena-warehouse -- portions` from the datasets `sources.toml`\n\
         pins and `units/count.toml`; the test `the_portions_report_is_current` fails when this file\n\
         differs from what that would write. Change the records, not this file.\n\n\
         USDA's portions give the grams in an amount of a unit (`silver.usda_portion`). Each\n\
         portion's unit is read from its text (`silver.usda_portion_unit`): the longest run of its\n\
         leading phrase's first words that is a mass or volume unit cooklang knows, or a count unit\n\
         `units/count.toml` lists. A portion no unit reads is listed below by its leading phrase,\n\
         not guessed.\n",
    );
    for d in &sources.dataset {
        let ds = literal(&d.id);
        let all = count(format!(
            "SELECT count(*) FROM silver.usda_portion WHERE dataset = {ds}"
        ))?;
        out.push_str(&format!(
            "\n## {} {} (`{}`): {all} portions\n\n| unit read as | portions |\n|---|---|\n",
            d.name, d.release, d.id
        ));
        for kind in ["mass", "volume", "count"] {
            let n = count(format!(
                "SELECT count(*) FROM silver.usda_portion_unit WHERE dataset = {ds} AND kind = {}",
                literal(kind)
            ))?;
            out.push_str(&format!("| {kind} | {n} |\n"));
        }
        let unread = count(format!(
            "SELECT count(*) FROM silver.usda_portion_unit WHERE dataset = {ds} AND kind IS NULL"
        ))?;
        out.push_str(&format!("| not read | {unread} |\n"));
    }
    out.push_str(
        "\n## Count units\n\n\
         Each count unit `units/count.toml` lists, and the portions it reads in each dataset. A\n\
         count unit has no grams of its own: its grams for a food come from that food's portion in\n\
         it.\n\n",
    );
    let datasets: Vec<&str> = sources.dataset.iter().map(|d| d.id.as_str()).collect();
    out.push_str(&format!(
        "| count unit | {} |\n|---|{}\n",
        datasets.join(" | "),
        "---|".repeat(datasets.len())
    ));
    let units = rows(
        conn,
        "SELECT unit FROM silver.count_unit ORDER BY unit",
        |r| r.get::<_, String>(0),
    )?;
    for unit in units {
        let mut cells = Vec::new();
        for ds in &datasets {
            cells.push(
                count(format!(
                    "SELECT count(*) FROM silver.usda_portion_unit WHERE dataset = {} AND kind = 'count' AND unit = {}",
                    literal(ds),
                    literal(&unit)
                ))?
                .to_string(),
            );
        }
        out.push_str(&format!("| {unit} | {} |\n", cells.join(" | ")));
    }
    out.push_str(
        "\n## Not read\n\n\
         Every leading phrase whose portions no unit reads (`gold.portion_unread`), most portions\n\
         first. A unit later in the text is not read as the portion's: \"serving 8 oz\" is a serving.\n\
         A count unit added to `units/count.toml` reads the phrases that name it.\n\n\
         | dataset | leading phrase | portions | example |\n|---|---|---|---|\n",
    );
    let unread = rows(
        conn,
        "SELECT dataset, phrase, portions, example FROM gold.portion_unread ORDER BY portions DESC, dataset, phrase",
        |r| {
            Ok(format!(
                "| {} | {} | {} | {} |",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?.replace('|', "\\|")
            ))
        },
    )?;
    for line in unread {
        out.push_str(&line);
        out.push('\n');
    }
    let disagree = rows(
        conn,
        "SELECT dataset, fdc_id, description, amount, unit, gram_weight, mass_ratio
         FROM gold.portion_mass_disagreement ORDER BY abs(mass_ratio - 1) DESC, dataset, id",
        |r| {
            Ok(format!(
                "| {} | {} | {} | {} {} | {} | {:.3} |",
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?.replace('|', "\\|"),
                r.get::<_, f64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, f64>(5)?,
                r.get::<_, f64>(6)?
            ))
        },
    )?;
    out.push_str(&format!(
        "\n## Mass portions that disagree with their unit\n\n\
         {} portions in a mass unit whose gram weight differs from what the unit defines by more\n\
         than 2% (`gold.portion_mass_disagreement`): a finding about USDA's data, kept as USDA gives\n\
         it. The ratio is the portion's grams over the unit's.\n",
        disagree.len()
    ));
    if !disagree.is_empty() {
        out.push_str(
            "\n| dataset | fdc_id | food | amount | grams | ratio |\n|---|---|---|---|---|---|\n",
        );
        for line in disagree {
            out.push_str(&line);
            out.push('\n');
        }
    }
    Ok(out)
}

/// `docs/data-catalogue.md`, as the warehouse writes it: every table and view, layer by layer, with
/// the comment it carries and its columns.
pub fn catalogue(conn: &Connection) -> Result<String, String> {
    let mut out = String::from(
        "# Data catalogue\n\n\
         Every table and view in the warehouse, as DuckDB describes them, with the comments\n\
         `crates/warehouse/schema.sql` and the loader give them. Generated by\n\
         `cargo run -p balaena-warehouse -- catalogue`; the test `the_catalogue_is_current` fails\n\
         when this file differs from what that would write. Change the schema, not this file.\n",
    );
    for schema in ["bronze", "silver", "gold"] {
        out.push_str(&format!("\n## {schema}\n"));
        let objects = rows(
            conn,
            &format!(
                "SELECT table_name, 'table', coalesce(comment, '') FROM duckdb_tables() \
                 WHERE schema_name = {s} AND NOT temporary \
                 UNION ALL \
                 SELECT view_name, 'view', coalesce(comment, '') FROM duckdb_views() \
                 WHERE schema_name = {s} AND NOT internal AND NOT temporary \
                 ORDER BY 1",
                s = literal(schema)
            ),
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )?;
        for (name, kind, comment) in objects {
            out.push_str(&format!("\n### {schema}.{name}\n\nA {kind}. {comment}\n\n"));
            let columns = rows(
                conn,
                &format!(
                    "SELECT column_name, data_type, coalesce(comment, '') FROM duckdb_columns() \
                     WHERE schema_name = {} AND table_name = {} ORDER BY column_index",
                    literal(schema),
                    literal(&name)
                ),
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )?;
            let (described, bare): (Vec<_>, Vec<_>) =
                columns.into_iter().partition(|(_, _, c)| !c.is_empty());
            if !described.is_empty() {
                out.push_str("| column | type | what it is |\n|---|---|---|\n");
                for (column, data_type, comment) in &described {
                    out.push_str(&format!("| {column} | {data_type} | {comment} |\n"));
                }
            }
            if !bare.is_empty() {
                let names: Vec<String> =
                    bare.iter().map(|(c, t, _)| format!("`{c}` {t}")).collect();
                let lead = if described.is_empty() {
                    "Columns"
                } else {
                    "\nOther columns"
                };
                out.push_str(&format!("{lead}: {}.\n", names.join(", ")));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use balaena_records::Dataset;
    use std::cell::Cell;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn dataset(bytes: &[u8]) -> Dataset {
        Dataset {
            id: "d".into(),
            publisher: "p".into(),
            name: "D".into(),
            release: "1".into(),
            url: "https://example.org/d.zip".into(),
            sha256: usda::sha256_hex(bytes),
            bytes: bytes.len() as u64,
        }
    }

    /// Two scratch dirs made at the same clock reading get different names. Making 256 in a row on
    /// one thread never collided even without the count, as each takes more than a microsecond;
    /// the collision came from tests on parallel threads, so the name is checked, not the timing.
    #[test]
    fn scratch_dirs_made_together_are_distinct() {
        assert_ne!(Scratch::name(7, 1_000, 0), Scratch::name(7, 1_000, 1));
        let dirs: Vec<Scratch> = (0..64).map(|_| Scratch::new().unwrap()).collect();
        let names: std::collections::BTreeSet<_> = dirs.iter().map(|d| d.0.clone()).collect();
        assert_eq!(names.len(), dirs.len());
    }

    #[test]
    fn what_counts_as_a_csv_table() {
        assert_eq!(csv_table("x/food.csv"), Ok(Some("usda_food".into())));
        assert_eq!(
            csv_table("food_nutrient.csv"),
            Ok(Some("usda_food_nutrient".into()))
        );
        assert_eq!(
            csv_table("x/Download & API Field Descriptions.xlsx"),
            Ok(None)
        );
        assert!(csv_table("x/Food Nutrient.csv").is_err());
        assert!(csv_table("x/.csv").is_err());
    }

    #[test]
    fn the_cache_follows_the_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(
            cache_dir_from(env(&[
                ("VBCI_CACHE", "/c"),
                ("XDG_CACHE_HOME", "/x"),
                ("HOME", "/h")
            ])),
            Ok(PathBuf::from("/c"))
        );
        assert_eq!(
            cache_dir_from(env(&[("XDG_CACHE_HOME", "/x"), ("HOME", "/h")])),
            Ok(PathBuf::from("/x/vbci"))
        );
        assert_eq!(
            cache_dir_from(env(&[("HOME", "/h")])),
            Ok(PathBuf::from("/h/.cache/vbci"))
        );
        assert!(cache_dir_from(env(&[])).is_err());
    }

    #[test]
    fn bytes_that_differ_from_the_pin_are_refused() {
        let good = b"the pinned zip";
        let d = dataset(good);
        assert_eq!(usda::verify(&d, good), Ok(()));
        let same_length = b"the pinned zap";
        assert!(
            usda::verify(&d, same_length)
                .unwrap_err()
                .contains("sha256")
        );
        assert!(usda::verify(&d, b"short").unwrap_err().contains("bytes"));

        let cache = Scratch::new().unwrap();
        let err = usda::fetch(std::slice::from_ref(&d), &cache.0, |_, _| {
            Ok(same_length.to_vec())
        })
        .unwrap_err();
        assert!(
            err.contains("sha256") && err.contains("downloaded from"),
            "{err}"
        );
        let left: Vec<_> = fs::read_dir(&cache.0).unwrap().collect();
        assert!(left.is_empty(), "a refused download left {left:?}");
    }

    #[test]
    fn a_cached_zip_that_differs_from_its_pin_is_refused() {
        let good = b"the pinned zip";
        let d = dataset(good);
        let cache = Scratch::new().unwrap();
        let path = usda::zip_path(&cache.0, &d);
        fs::write(&path, b"the pinned zap").unwrap();
        let called = Cell::new(false);
        let err = usda::fetch(std::slice::from_ref(&d), &cache.0, |_, _| {
            called.set(true);
            Ok(good.to_vec())
        })
        .unwrap_err();
        assert!(err.contains("cached at"), "{err}");
        assert!(
            !called.get(),
            "a cached zip that differs was replaced by a download"
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            b"the pinned zap",
            "the cached zip was changed"
        );
        let out = Scratch::new().unwrap();
        assert!(
            usda::extract(&cache.0, &d, &out.0)
                .unwrap_err()
                .contains("sha256")
        );
    }

    #[test]
    fn a_good_download_is_cached_under_its_sha256() {
        let good = b"the pinned zip";
        let d = dataset(good);
        let cache = Scratch::new().unwrap();
        let calls = Cell::new(0);
        let get = |url: &str, limit: u64| {
            calls.set(calls.get() + 1);
            assert_eq!((url, limit), (d.url.as_str(), d.bytes));
            Ok(good.to_vec())
        };
        let first = usda::fetch(std::slice::from_ref(&d), &cache.0, get).unwrap();
        assert_eq!(first, [("d".to_string(), usda::Fetched::Downloaded)]);
        assert_eq!(fs::read(usda::zip_path(&cache.0, &d)).unwrap(), good);
        let second = usda::fetch(std::slice::from_ref(&d), &cache.0, get).unwrap();
        assert_eq!(second, [("d".to_string(), usda::Fetched::Cached)]);
        assert_eq!(calls.get(), 1);
    }

    /// A pinned Wikidata revision is downloaded once and cached under its pin: item, revision and
    /// sha256. Content that differs from its pin is refused, neither written when downloaded nor
    /// replaced when cached, and reading it for the load refuses it too; a pin whose revision was
    /// mistyped misses the cache and is checked against the answer.
    #[test]
    fn a_wikidata_revision_is_cached_under_its_pin_and_refused_where_it_differs() {
        let content = r#"{"type":"item","id":"Q1","claims":{}}"#;
        let w = balaena_records::WikidataItem {
            item: "Q1".into(),
            revision: 7,
            sha256: usda::sha256_hex(content.as_bytes()),
            bytes: content.len() as u64,
        };
        let answer = |c: &str| {
            format!(
                r#"{{"query":{{"pages":[{{"title":"Q1","revisions":[{{"revid":7,"slots":{{"main":{{"content":{}}}}}}}]}}]}}}}"#,
                serde_json::to_string(c).unwrap()
            )
            .into_bytes()
        };
        let cache = Scratch::new().unwrap();
        let calls = Cell::new(0);
        let get = |url: &str, limit: u64| {
            calls.set(calls.get() + 1);
            assert_eq!(
                (url, limit),
                (wikidata::url(&w).as_str(), wikidata::answer_limit(&w))
            );
            Ok(answer(content))
        };
        let first = wikidata::fetch(std::slice::from_ref(&w), &cache.0, get).unwrap();
        assert_eq!(first, [("Q1".to_string(), usda::Fetched::Downloaded)]);
        let path = wikidata::content_path(&cache.0, &w);
        assert_eq!(fs::read_to_string(&path).unwrap(), content);
        let second = wikidata::fetch(std::slice::from_ref(&w), &cache.0, get).unwrap();
        assert_eq!(second, [("Q1".to_string(), usda::Fetched::Cached)]);
        assert_eq!(calls.get(), 1);
        assert_eq!(wikidata::read(&cache.0, &w).unwrap(), content);

        let other = content.replace("{}", "[]");
        assert_eq!(other.len(), content.len());
        let empty = Scratch::new().unwrap();
        let err = wikidata::fetch(std::slice::from_ref(&w), &empty.0, |_, _| {
            Ok(answer(&other))
        })
        .unwrap_err();
        assert!(
            err.contains("sha256") && err.contains("downloaded from"),
            "{err}"
        );
        let left: Vec<_> = fs::read_dir(&empty.0).unwrap().collect();
        assert!(left.is_empty(), "a refused download left {left:?}");
        let err = wikidata::fetch(std::slice::from_ref(&w), &empty.0, |_, _| {
            Ok(answer(&format!("{content} ")))
        })
        .unwrap_err();
        assert!(err.contains("bytes, but sources.toml pins"), "{err}");

        fs::write(&path, &other).unwrap();
        let called = Cell::new(false);
        let err = wikidata::fetch(std::slice::from_ref(&w), &cache.0, |_, _| {
            called.set(true);
            Ok(answer(content))
        })
        .unwrap_err();
        assert!(err.contains("sha256") && err.contains("cached at"), "{err}");
        assert!(!called.get(), "cached content that differs was replaced");
        assert_eq!(fs::read_to_string(&path).unwrap(), other);
        let err = wikidata::read(&cache.0, &w).unwrap_err();
        assert!(err.contains("sha256") && err.contains("cached at"), "{err}");

        let good = Scratch::new().unwrap();
        wikidata::fetch(std::slice::from_ref(&w), &good.0, |_, _| {
            Ok(answer(content))
        })
        .unwrap();
        let mistyped = balaena_records::WikidataItem {
            revision: 8,
            ..w.clone()
        };
        let err = wikidata::fetch(std::slice::from_ref(&mistyped), &good.0, |_, _| {
            Ok(answer(content))
        })
        .unwrap_err();
        assert!(err.contains("the answer is revision 7"), "{err}");
    }

    /// `load` reads each pinned revision through its pin too: content of the pinned length with
    /// other bytes, cached where the revision should be, stops the load.
    #[test]
    fn loading_refuses_cached_wikidata_content_that_differs() {
        let (sources, _) = Sources::load(&repo_root()).unwrap();
        let real = cache_dir().unwrap();
        let cache = Scratch::new().unwrap();
        for d in &sources.dataset {
            fs::copy(usda::zip_path(&real, d), usda::zip_path(&cache.0, d)).unwrap();
        }
        for w in &sources.wikidata {
            fs::copy(
                wikidata::content_path(&real, w),
                wikidata::content_path(&cache.0, w),
            )
            .unwrap();
        }
        let w = &sources.wikidata[0];
        fs::write(
            wikidata::content_path(&cache.0, w),
            vec![b' '; w.bytes as usize],
        )
        .unwrap();
        let Err(err) = load(&repo_root(), &cache.0) else {
            panic!("cached content that differs from its pin was loaded");
        };
        assert!(
            err.contains(&format!("wikidata {}: sha256", w.item)) && err.contains("cached at"),
            "{err}"
        );
    }

    /// `load` checks each cached zip against its pin too: a file of the pinned length with other
    /// bytes, sitting where the zip should be, stops the load.
    #[test]
    fn loading_refuses_a_cached_zip_that_differs() {
        let (sources, _) = Sources::load(&repo_root()).unwrap();
        let cache = Scratch::new().unwrap();
        for d in &sources.dataset {
            fs::write(usda::zip_path(&cache.0, d), vec![0u8; d.bytes as usize]).unwrap();
        }
        let Err(err) = load(&repo_root(), &cache.0) else {
            panic!("a cached zip that differs from its pin was loaded");
        };
        assert!(err.contains("sha256") && err.contains("cached at"), "{err}");
    }

    /// A file in a zip whose path climbs out of its folder (`../`) is refused, and nothing is
    /// written outside the folder.
    #[test]
    fn a_member_that_would_extract_outside_its_folder_is_refused() {
        use std::io::Write;
        use zip::write::{SimpleFileOptions, ZipWriter};
        let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let stored =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer.start_file("../outside.csv", stored).unwrap();
        writer.write_all(b"a,b\n1,2\n").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let d = dataset(&bytes);
        let cache = Scratch::new().unwrap();
        fs::write(usda::zip_path(&cache.0, &d), &bytes).unwrap();
        let out = Scratch::new().unwrap();
        let dir = out.0.join("into");
        let err = usda::extract(&cache.0, &d, &dir).unwrap_err();
        assert!(err.contains("outside its folder"), "{err}");
        assert!(!dir.join("outside.csv").exists() && !out.0.join("outside.csv").exists());
    }

    /// Nutrients for the synthetic bronze below.
    const SYNTHETIC_NUTRIENTS: &str = r#"
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

    /// Diet records for the synthetic bronze below, in the order of `Diet::FILES`.
    const SYNTHETIC_DIET: [&str; 4] = [
        "[[category]]\ndescription = \"Vegetables\"\nclass = \"plant\"\n
[[category]]\ndescription = \"Dairy\"\nclass = \"dairy_and_egg\"\n
[[category]]\ndescription = \"Poultry\"\nclass = \"flesh\"\n
[[category]]\ndescription = \"Fats\"\nclass = \"mixed\"\n",
        "[[taxon]]\nid = \"NCBITaxon_33208\"\nname = \"Metazoa\"\ngroup = \"animal\"\n
[[taxon]]\nid = \"NCBITaxon_35493\"\nname = \"Streptophyta\"\ngroup = \"plant\"\n",
        "[[rule]]\ncategory = \"Dairy\"\nstarts_with = \"Cheese\"\nvegetarian = \"unknown\"\nwhy = \"Rennet.\"\n
[[rule]]\ncategory = \"Fats\"\nstarts_with = \"Oil,\"\nvegan = \"yes\"\nvegetarian = \"yes\"\nwhy = \"Plant oils.\"\n
[[rule]]\ncategory = \"Vegetables\"\nstarts_with = \"B\"\nvegan = \"yes\"\nvegetarian = \"yes\"\nwhy = \"Over the organism.\"\n",
        "[[food]]\nfdc_id = 7\ndescription = \"Lard\"\nvegan = \"no\"\nvegetarian = \"no\"\nwhy = \"Carcass fat.\"\n
[[food]]\nfdc_id = 8\ndescription = \"Potatoes, mashed, with milk\"\nvegan = \"no\"\nwhy = \"Milk.\"\n
[[food]]\nfdc_id = 6\ndescription = \"Oil, olive\"\nvegan = \"unknown\"\nwhy = \"Over the rule.\"\n
[[food]]\nfdc_id = 4\ndescription = \"Cheese, parmesan\"\nvegetarian = \"yes\"\nwhy = \"Vegetarian rennet, over the rule.\"\n",
    ];

    /// A warehouse whose bronze is a few rows written here, with silver loaded from them by
    /// `nutrients` and `SYNTHETIC_DIET`. Food 3 is a sample food, out of silver's scope. Nutrient
    /// rows: 1-2 give food 1 energy by 208 and 958; 3 gives food 2 energy by 958 only; 4 gives food
    /// 2 sugars by 269.3 only; 5 gives food 1 sugars by 269, and 6 repeats it; 7 and 8 give food 2
    /// different sugars by 269; 9 names a nutrient nutrient.csv lacks; 10 has no amount; 11 is the
    /// sample food's. Portions: p1 to p4, p7 and p9 are kept with their units read or not, p5 has
    /// amount 0, p8 gram weight 0, p6 is the sample food's; p7 weighs three times its ounce and p9
    /// less than its pound, and p4's measure unit is read before its modifier. Foods 4-10 sit in four categories for the diet; foods 2 and 10 record an
    /// animal organism, food 6 a plant, food 1 NA. A naming rule matches food 2, and exceptions
    /// override the rules matching foods 4 and 6, so each layer is seen over the one below it.
    /// Foods 2 and 6 record their own organism, food 6 twice, and food 10 NA; olive oil's organism
    /// is read from food 6, and a-food's from Wikidata item Q1 through Q2.
    fn synthetic(nutrients: &str) -> Result<Connection, String> {
        synthetic_with(nutrients, SYNTHETIC_DIET)
    }

    /// Ingredients for the synthetic bronze, as (file name, text).
    const SYNTHETIC_INGREDIENTS: [(&str, &str); 2] = [
        (
            "a-food.toml",
            "name = \"A\"\naliases = [\"a-food\", \"ay\"]\n\n[organism]\ntaxon = \"NCBITaxon_111\"\nwikidata = \"Q1\"\n\n[origin]\nharvested = true\n\n[[form]]\nstate = \"raw\"\nfdc_id = 1\ndescription = \"A\"\n",
        ),
        (
            "olive-oil.toml",
            "name = \"Olive oil\"\naliases = [\"olive oil\"]\n\n[organism]\ntaxon = \"NCBITaxon_4146\"\nfdc_id = 6\ndescription = \"Oil, olive\"\n\n[origin]\nnone = \"Pressed from olives.\"\n\n[[form]]\nstate = \"as_sold\"\nfdc_id = 6\ndescription = \"Oil, olive\"\n",
        ),
    ];

    /// A Wikidata item's statements, as (property, value, rank).
    type Claims<'a> = &'a [(&'a str, &'a str, &'a str)];

    /// One refusal case for crops: the ingredient files, the pinned items, the crop's text, and
    /// what the refusal must say.
    type CropCase<'a> = (
        &'a [(&'a str, &'a str)],
        Vec<(String, String)>,
        String,
        &'a str,
    );

    /// A Wikidata item's content, as Wikidata stores it, with `statements` as its claims.
    fn wikidata_item(id: &str, statements: &[(&str, &str, &str)]) -> String {
        let mut claims: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for (property, value, rank) in statements {
            let datavalue = match *property {
                "P685" | "P225" => format!("{{\"value\":\"{value}\",\"type\":\"string\"}}"),
                _ => format!(
                    "{{\"value\":{{\"entity-type\":\"item\",\"id\":\"{value}\"}},\"type\":\"wikibase-entityid\"}}"
                ),
            };
            claims.entry(property).or_default().push(format!(
                "{{\"mainsnak\":{{\"snaktype\":\"value\",\"property\":\"{property}\",\"datavalue\":{datavalue}}},\"type\":\"statement\",\"rank\":\"{rank}\"}}"
            ));
        }
        let claims: Vec<String> = claims
            .iter()
            .map(|(p, s)| format!("\"{p}\":[{}]", s.join(",")))
            .collect();
        format!(
            "{{\"type\":\"item\",\"id\":\"{id}\",\"labels\":{{}},\"claims\":{{{}}}}}",
            claims.join(",")
        )
    }

    /// The synthetic warehouse's pinned Wikidata items: Q1 is a product of Q2, whose NCBI id is 111.
    fn synthetic_wikidata() -> Vec<(String, String)> {
        vec![
            (
                "Q1".into(),
                wikidata_item("Q1", &[("P1582", "Q2", "normal")]),
            ),
            (
                "Q2".into(),
                wikidata_item(
                    "Q2",
                    &[("P685", "111", "normal"), ("P225", "A b", "normal")],
                ),
            ),
        ]
    }

    /// `synthetic_wikidata` and the items the synthetic crops read: Q3, a variety whose parent
    /// taxon is Q10, which is NCBI id 111 too. Q3 is read only as a crop's own item and Q10 only as
    /// a crop's parent, so each way of reading an item is the only one that reads some item.
    fn crop_wikidata(more: &[(&str, Claims)]) -> Vec<(String, String)> {
        let mut items = synthetic_wikidata();
        items.push((
            "Q3".into(),
            wikidata_item(
                "Q3",
                &[
                    ("P685", "112", "normal"),
                    ("P225", "A b var. c", "normal"),
                    ("P171", "Q10", "normal"),
                ],
            ),
        ));
        items.push((
            "Q10".into(),
            wikidata_item(
                "Q10",
                &[("P685", "111", "normal"), ("P225", "A b", "normal")],
            ),
        ));
        for (q, statements) in more {
            items.push((q.to_string(), wikidata_item(q, statements)));
        }
        items
    }

    /// Crops for the synthetic warehouse: a-plant is a-food's organism, a-variety one step below it.
    const SYNTHETIC_CROPS: [(&str, &str); 2] = [
        (
            "a-plant.toml",
            "name = \"A plant\"\nwikidata = \"Q2\"\ntaxon = \"NCBITaxon_111\"\nscientific_name = \"A b\"\ningredient = \"a-food\"\nstate = \"raw\"\n",
        ),
        (
            "a-variety.toml",
            "name = \"A variety\"\nwikidata = \"Q3\"\ntaxon = \"NCBITaxon_112\"\nscientific_name = \"A b var. c\"\ningredient = \"a-food\"\nstate = \"raw\"\n",
        ),
    ];

    fn synthetic_crops(
        ingredients: &[(&str, &str)],
        wikidata: &[(String, String)],
        crops: &[(&str, &str)],
    ) -> Result<Connection, String> {
        synthetic_pinned(
            SYNTHETIC_NUTRIENTS,
            SYNTHETIC_DIET,
            ingredients,
            SYNTHETIC_COUNT_UNITS,
            wikidata,
            crops,
        )
    }

    /// Count units for the synthetic bronze.
    const SYNTHETIC_COUNT_UNITS: &str = "[[unit]]\nkey = \"medium\"\nnames = [\"medium\"]\n";

    fn synthetic_with(nutrients: &str, diet: [&str; 4]) -> Result<Connection, String> {
        synthetic_full(
            nutrients,
            diet,
            &SYNTHETIC_INGREDIENTS,
            SYNTHETIC_COUNT_UNITS,
        )
    }

    fn synthetic_full(
        nutrients: &str,
        diet: [&str; 4],
        ingredients: &[(&str, &str)],
        counts: &str,
    ) -> Result<Connection, String> {
        synthetic_pinned(
            nutrients,
            diet,
            ingredients,
            counts,
            &synthetic_wikidata(),
            &[],
        )
    }

    /// The synthetic warehouse with `wikidata` as its pinned items, (item, content), and `crops`
    /// as its crops, (file name, text).
    fn synthetic_pinned(
        nutrients: &str,
        diet: [&str; 4],
        ingredients: &[(&str, &str)],
        counts: &str,
        wikidata: &[(String, String)],
        crops: &[(&str, &str)],
    ) -> Result<Connection, String> {
        let conn = Connection::open_in_memory().map_err(sql)?;
        conn.execute_batch(SCHEMA).map_err(sql)?;
        conn.execute_batch(&format!(
            "INSERT INTO bronze.usda_zip VALUES ('d', 'https://example.org/d.zip', '{}', 1);
             INSERT INTO bronze.record VALUES ('nutrients.toml', 'synthetic');
             CREATE TABLE bronze.usda_food_category (dataset VARCHAR, id VARCHAR, code VARCHAR, description VARCHAR);
             INSERT INTO bronze.usda_food_category VALUES ('d', '1', '0100', 'Vegetables'),
               ('d', '2', '0200', 'Dairy'), ('d', '3', '0300', 'Poultry'), ('d', '4', '0400', 'Fats');
             CREATE TABLE bronze.usda_food (dataset VARCHAR, fdc_id VARCHAR, data_type VARCHAR, description VARCHAR, food_category_id VARCHAR);
             INSERT INTO bronze.usda_food VALUES
               ('d', '1', 'sr_legacy_food', 'A', '1'), ('d', '2', 'foundation_food', 'B', '1'),
               ('d', '3', 'sample_food', 'C', '1'), ('d', '4', 'sr_legacy_food', 'Cheese, parmesan', '2'),
               ('d', '5', 'sr_legacy_food', 'Chicken, raw', '3'), ('d', '6', 'foundation_food', 'Oil, olive', '4'),
               ('d', '7', 'sr_legacy_food', 'Lard', '4'), ('d', '8', 'sr_legacy_food', 'Potatoes, mashed, with milk', '1'),
               ('d', '9', 'sr_legacy_food', 'Cheese sauce', '4'), ('d', '10', 'foundation_food', 'D', '1');
             CREATE TABLE bronze.usda_food_attribute (dataset VARCHAR, id VARCHAR, fdc_id VARCHAR, food_attribute_type_id VARCHAR, name VARCHAR, value VARCHAR);
             INSERT INTO bronze.usda_food_attribute VALUES
               ('d', '1', '2', NULL, 'NCBI Taxon Parent', 'http://purl.obolibrary.org/obo/NCBITaxon_33208'),
               ('d', '2', '2', NULL, 'NCBI Taxon Parent', 'http://purl.obolibrary.org/obo/NCBITaxon_33208'),
               ('d', '3', '6', NULL, 'NCBI Taxon Parent', 'http://purl.obolibrary.org/obo/NCBITaxon_35493'),
               ('d', '6', '10', NULL, 'NCBI Taxon Parent', 'http://purl.obolibrary.org/obo/NCBITaxon_33208'),
               ('d', '4', '1', NULL, 'NCBI Taxon Parent', 'NA'),
               ('d', '5', '1', '1000', NULL, 'Ay, a-food, plain a'),
               ('d', '7', '6', NULL, 'FoodOn Ontology ID For FDC Item', 'http://purl.obolibrary.org/obo/FOODON_03301826'),
               ('d', '8', '6', NULL, 'FoodOn Ontology ID For FDC Item', 'http://purl.obolibrary.org/obo/FOODON_03301826'),
               ('d', '9', '6', NULL, 'NCBI Taxon', 'http://purl.obolibrary.org/obo/NCBITaxon_4146'),
               ('d', '10', '6', NULL, 'NCBI Taxon', 'http://purl.obolibrary.org/obo/NCBITaxon_4146'),
               ('d', '11', '2', NULL, 'NCBI Taxon', 'http://purl.obolibrary.org/obo/NCBITaxon_9913'),
               ('d', '12', '10', NULL, 'NCBI Taxon', 'NA');
             CREATE TABLE bronze.usda_measure_unit (dataset VARCHAR, id VARCHAR, name VARCHAR);
             INSERT INTO bronze.usda_measure_unit VALUES ('d', '9999', 'undetermined'), ('d', '1000', 'tablespoon');
             CREATE TABLE bronze.usda_food_portion (dataset VARCHAR, id VARCHAR, fdc_id VARCHAR, amount VARCHAR, measure_unit_id VARCHAR, modifier VARCHAR, gram_weight VARCHAR);
             INSERT INTO bronze.usda_food_portion VALUES
               ('d', 'p1', '1', '1', '9999', 'cup, chopped', '160'), ('d', 'p2', '1', '2', '9999', 'oz', '57'),
               ('d', 'p3', '5', '1', '9999', 'steak', '200'), ('d', 'p4', '6', '1', '1000', 'chopped', '14'),
               ('d', 'p5', '1', '0', '9999', 'cup', '100'), ('d', 'p6', '3', '1', '9999', 'cup', '1'),
               ('d', 'p7', '2', '1', '9999', 'oz', '86'), ('d', 'p8', '2', '1', '9999', 'cup', '0'),
               ('d', 'p9', '2', '1', '9999', 'lb', '400');
             CREATE TABLE bronze.usda_nutrient (dataset VARCHAR, id VARCHAR, name VARCHAR, unit_name VARCHAR, nutrient_nbr VARCHAR);
             INSERT INTO bronze.usda_nutrient VALUES
               ('d', '1008', 'Energy', 'KCAL', '208'), ('d', '2048', 'Energy (Atwater)', 'KCAL', '958'),
               ('d', '2000', 'Sugars, Total', 'G', '269'), ('d', '1063', 'Sugars, NLEA', 'G', '269.3'),
               ('d', '1062', 'Energy', 'kJ', '268'), ('d', '1024', 'Specific Gravity', 'SP_GR', NULL);
             CREATE TABLE bronze.usda_food_nutrient (dataset VARCHAR, id VARCHAR, fdc_id VARCHAR, nutrient_id VARCHAR, amount VARCHAR);
             INSERT INTO bronze.usda_food_nutrient VALUES
               ('d', '1', '1', '1008', '100'), ('d', '2', '1', '2048', '90'), ('d', '3', '2', '2048', '50'),
               ('d', '4', '2', '1063', '7'), ('d', '5', '1', '2000', '3'), ('d', '6', '1', '2000', '3'),
               ('d', '7', '2', '2000', '1'), ('d', '8', '2', '2000', '2'), ('d', '9', '1', '9999', '5'),
               ('d', '10', '1', '1008', NULL), ('d', '11', '3', '1008', '1');",
            "a".repeat(64)
        ))
        .map_err(sql)?;
        for (n, (item, content)) in wikidata.iter().enumerate() {
            conn.execute(
                "INSERT INTO bronze.wikidata_revision VALUES (?, ?, ?, ?, ?)",
                params![
                    item,
                    n as i64 + 1,
                    usda::sha256_hex(content.as_bytes()),
                    content.len() as i64,
                    content
                ],
            )
            .map_err(sql)?;
        }
        load_silver(&conn, &Nutrients::parse(nutrients)?)?;
        load_diet(&conn, &Diet::parse(diet[0], diet[1], diet[2], diet[3])?)?;
        let files: Vec<(String, String)> = ingredients
            .iter()
            .map(|(f, t)| (f.to_string(), t.to_string()))
            .collect();
        let ingredients = Ingredients::parse(&files)?;
        let crops: Vec<(String, String)> = crops
            .iter()
            .map(|(f, t)| (f.to_string(), t.to_string()))
            .collect();
        let crops = Crops::parse(&crops)?;
        load_ingredients(&conn, &ingredients)?;
        load_organisms(&conn, &ingredients, &crops)?;
        load_portions(&conn, &CountUnits::parse(counts)?)?;
        Ok(conn)
    }

    /// Portions of silver foods are kept with their unit read (p1 cup, p2 and p7 oz, p9 lb, p4
    /// its measure unit "tablespoon", not its modifier "chopped") or listed as not read (p3
    /// "steak"); one with amount 0 (p5) and one with gram weight 0 (p8) are rejected; the sample
    /// food's is out of scope (p6). Grams per unit, grams per millilitre, and a mass unit's
    /// agreement with its definition follow: p7 and p9 disagree, one each way, and are kept and
    /// listed. The count units are in silver with their names.
    #[test]
    fn each_portion_s_unit_is_read_or_listed() {
        let w = synthetic(SYNTHETIC_NUTRIENTS).unwrap();
        assert_eq!(
            strings(
                &w,
                "SELECT id || ' ' || phrase || ' ' || coalesce(kind, '-') || ' ' || coalesce(unit, '-') FROM silver.usda_portion_unit ORDER BY id"
            ),
            [
                "p1 cup volume c",
                "p2 oz mass oz",
                "p3 steak - -",
                "p4 tablespoon volume tbsp",
                "p7 oz mass oz",
                "p9 lb mass lb"
            ]
        );
        assert_eq!(
            strings(
                &w,
                "SELECT id || ': ' || reason FROM silver.usda_rejected WHERE source = 'usda_food_portion' ORDER BY id"
            ),
            [
                "p5: amount is 0.0, so no grams per unit",
                "p8: gram weight is 0.0"
            ]
        );
        assert_eq!(
            strings(
                &w,
                "SELECT id || ' ' || round(grams_per_unit, 3) || ' ' || coalesce(round(grams_per_ml, 3)::VARCHAR, '-') || ' ' || coalesce(round(mass_ratio, 3)::VARCHAR, '-') FROM gold.portion ORDER BY id"
            ),
            [
                "p1 160.0 0.676 -",
                "p2 28.5 - 1.005",
                "p4 14.0 0.947 -",
                "p7 86.0 - 3.034",
                "p9 400.0 - 0.882"
            ]
        );
        assert_eq!(
            strings(
                &w,
                "SELECT phrase || ' ' || portions FROM gold.portion_unread"
            ),
            ["steak 1"]
        );
        assert_eq!(
            strings(
                &w,
                "SELECT id || ' ' || description FROM gold.portion_mass_disagreement ORDER BY id"
            ),
            ["p7 B", "p9 B"]
        );
        assert_eq!(
            strings(
                &w,
                "SELECT n.name || ' ' || u.unit || ' ' || u.whole || ' ' || u.record FROM silver.count_unit u JOIN silver.count_unit_name n USING (unit)"
            ),
            ["medium medium false units/count.toml"]
        );
    }

    /// A count unit cooklang knows as a mass or volume unit stops the load: "cup" must mean one
    /// thing in a recipe and in USDA's text.
    #[test]
    fn the_load_refuses_a_count_unit_cooklang_knows() {
        let clash = "[[unit]]\nkey = \"cup\"\nnames = [\"cups\"]\n";
        let Err(err) = synthetic_full(
            SYNTHETIC_NUTRIENTS,
            SYNTHETIC_DIET,
            &SYNTHETIC_INGREDIENTS,
            clash,
        ) else {
            panic!("loaded a count unit named \"cups\"");
        };
        assert!(
            err.contains("count unit cup's name \"cups\" is a unit cooklang knows"),
            "{err}"
        );
    }

    fn strings(conn: &Connection, query: &str) -> Vec<String> {
        rows(conn, query, |r| r.get::<_, String>(0)).unwrap()
    }

    #[test]
    fn silver_keeps_a_row_or_rejects_it_with_its_reason() {
        let w = synthetic(SYNTHETIC_NUTRIENTS).unwrap();
        assert_eq!(
            strings(
                &w,
                "SELECT source_id FROM silver.usda_food_nutrient ORDER BY CAST(source_id AS INTEGER)"
            ),
            ["1", "2", "3", "4", "5"]
        );
        assert_eq!(
            strings(
                &w,
                "SELECT id || ': ' || reason FROM silver.usda_rejected WHERE source = 'usda_food_nutrient' ORDER BY CAST(id AS INTEGER)"
            ),
            [
                "6: repeats row 5: same food, nutrient and amount",
                "7: another row gives food 2 a different amount of nutrient 2000",
                "8: another row gives food 2 a different amount of nutrient 2000",
                "9: nutrient 9999 is not in its dataset's nutrient.csv",
                "10: no amount",
            ]
        );
        assert_eq!(
            strings(
                &w,
                "SELECT CAST(fdc_id AS VARCHAR) FROM silver.usda_food ORDER BY fdc_id"
            ),
            ["1", "2", "4", "5", "6", "7", "8", "9", "10"]
        );
    }

    #[test]
    fn the_first_listed_code_with_a_value_is_used() {
        let w = synthetic(SYNTHETIC_NUTRIENTS).unwrap();
        assert_eq!(
            strings(
                &w,
                "SELECT fdc_id || ' ' || nutrient || ' ' || amount || ' ' || code FROM gold.food_nutrient ORDER BY fdc_id, nutrient"
            ),
            [
                "1 energy 100.0 208",
                "1 sugars 3.0 269",
                "2 energy 50.0 958",
                "2 sugars 7.0 269.3"
            ]
        );
    }

    /// 269.3 is not 269: with only 269 listed, food 2 (which has sugars by 269.3, and only
    /// disagreeing rows by 269) has no sugars at all.
    #[test]
    fn codes_are_matched_exactly_not_rounded() {
        let only_269 = SYNTHETIC_NUTRIENTS.replace("[\"269\", \"269.3\"]", "[\"269\"]");
        let w = synthetic(&only_269).unwrap();
        assert_eq!(
            strings(
                &w,
                "SELECT fdc_id || ' ' || code FROM gold.food_nutrient WHERE nutrient = 'sugars'"
            ),
            ["1 269"]
        );
    }

    #[test]
    fn a_nutrient_code_usda_does_not_have_is_refused() {
        for code in ["2690", "269.30", "269.0"] {
            let wrong = SYNTHETIC_NUTRIENTS.replace("\"269.3\"", &format!("\"{code}\""));
            let Err(err) = synthetic(&wrong) else {
                panic!("loaded");
            };
            assert!(
                err.contains(&format!("code {code} is not in d's nutrient.csv")),
                "{err}"
            );
        }
    }

    #[test]
    fn a_nutrient_whose_unit_differs_is_refused() {
        let wrong = SYNTHETIC_NUTRIENTS.replace("unit = \"KCAL\"", "unit = \"kJ\"");
        let Err(err) = synthetic(&wrong) else {
            panic!("loaded");
        };
        assert!(
            err.contains("energy's code 208 has unit KCAL in d, not kJ"),
            "{err}"
        );
    }

    /// Silver's keys refuse a second food with one fdc_id, a food in no category, an amount for a
    /// nutrient its dataset lacks or a second amount for one food and nutrient, one code in two
    /// nutrients, a rejected row with a blank reason, a crop harvested in a state other than raw or
    /// dry, and a count unit that says neither that it is whole nor that it is not; the control
    /// rows beside each are accepted.
    #[test]
    fn the_database_refuses_what_silver_s_keys_refuse() {
        let w = synthetic(SYNTHETIC_NUTRIENTS).unwrap();
        let accepted = |sql: &str| {
            w.execute_batch(&format!("BEGIN; {sql}; ROLLBACK;"))
                .map_err(|e| {
                    let _ = w.execute_batch("ROLLBACK");
                    e.to_string()
                })
        };
        let cases = [
            (
                "INSERT INTO silver.usda_food VALUES (99, 'd', 'sr_legacy_food', 'X', 1)",
                true,
            ),
            (
                "INSERT INTO silver.usda_food VALUES (1, 'd', 'sr_legacy_food', 'X', 1)",
                false,
            ),
            (
                "INSERT INTO silver.usda_food VALUES (99, 'd', 'sr_legacy_food', 'X', 7)",
                false,
            ),
            (
                "INSERT INTO silver.usda_food VALUES (99, 'd', 'sample_food', 'X', 1)",
                false,
            ),
            (
                "INSERT INTO silver.usda_food_nutrient VALUES (2, 'd', 1008, 1.0, 'x')",
                true,
            ),
            (
                "INSERT INTO silver.usda_food_nutrient VALUES (2, 'd', 9999, 1.0, 'x')",
                false,
            ),
            (
                "INSERT INTO silver.usda_food_nutrient VALUES (1, 'd', 1008, 1.0, 'x')",
                false,
            ),
            (
                "INSERT INTO silver.nutrient_code VALUES ('energy', 3, '268')",
                true,
            ),
            (
                "INSERT INTO silver.nutrient_code VALUES ('energy', 3, '269')",
                false,
            ),
            (
                "INSERT INTO silver.usda_rejected VALUES ('t', 'd', 'x', 'a reason')",
                true,
            ),
            (
                "INSERT INTO silver.usda_rejected VALUES ('t', 'd', 'x', ' ')",
                false,
            ),
            (
                "INSERT INTO silver.crop VALUES ('c', 'C', 'Q2', 'NCBITaxon_111', 'A b', 'a-food', 'raw', NULL, 'x')",
                true,
            ),
            (
                "INSERT INTO silver.crop VALUES ('c', 'C', 'Q2', 'NCBITaxon_111', 'A b', 'olive-oil', 'as_sold', NULL, 'x')",
                false,
            ),
            (
                "INSERT INTO silver.count_unit VALUES ('x', true, 'x')",
                true,
            ),
            (
                "INSERT INTO silver.count_unit VALUES ('x', NULL, 'x')",
                false,
            ),
        ];
        for (sql, ok) in cases {
            assert_eq!(accepted(sql).is_ok(), ok, "{sql}: {:?}", accepted(sql));
        }
    }

    fn diet_of(w: &Connection) -> Vec<String> {
        strings(
            w,
            "SELECT fdc_id || ' ' || vegan || ' (' || vegan_basis || ') ' || vegetarian || ' (' || vegetarian_basis || ')' \
             FROM gold.food_diet ORDER BY fdc_id",
        )
    }

    /// Each food's answers come from the first that applies: an exception, a naming rule, an
    /// animal organism, the category, each for the diets it gives. An exception overrides a rule
    /// (food 6's vegan, food 4's vegetarian) and a rule an organism (food 2); a rule applies within
    /// its category only
    /// (food 9, "Cheese sauce", is in Fats, not Dairy); an animal organism makes a food not vegan,
    /// and a plant category's food vegetarian: unknown (food 10); a plant organism changes nothing.
    #[test]
    fn each_food_s_answers_come_from_the_first_layer_that_applies() {
        let w = synthetic(SYNTHETIC_NUTRIENTS).unwrap();
        assert_eq!(
            diet_of(&w),
            [
                "1 yes (category: plant) yes (category: plant)",
                "2 yes (rule: B) yes (rule: B)",
                "4 no (category: dairy_and_egg) yes (exception)",
                "5 no (category: flesh) no (category: flesh)",
                "6 unknown (exception) yes (rule: Oil,)",
                "7 no (exception) no (exception)",
                "8 no (exception) yes (category: plant)",
                "9 unknown (category: mixed) unknown (category: mixed)",
                "10 no (organism: animal) unknown (organism: animal)",
            ]
        );
        assert_eq!(
            strings(
                &w,
                "SELECT CAST(fdc_id AS VARCHAR) || ' ' || taxon FROM silver.usda_food_taxon ORDER BY fdc_id"
            ),
            [
                "2 NCBITaxon_33208",
                "6 NCBITaxon_35493",
                "10 NCBITaxon_33208"
            ]
        );
    }

    /// The diet records' comments rule these out, and the load stops on each, naming it.
    #[test]
    fn the_load_refuses_what_the_diet_records_rule_out() {
        let [categories, taxa, rules, exceptions] = SYNTHETIC_DIET;
        let fats = "[[category]]\ndescription = \"Fats\"\nclass = \"mixed\"\n";
        let animal =
            "[[taxon]]\nid = \"NCBITaxon_33208\"\nname = \"Metazoa\"\ngroup = \"animal\"\n";
        let oil = "[[rule]]\ncategory = \"Fats\"\nstarts_with = \"Oil,\"\nvegan = \"yes\"\nvegetarian = \"yes\"\nwhy = \"Plant oils.\"\n";
        assert!(rules.contains(oil));
        let cases: [([String; 4], &str); 8] = [
            (
                [
                    categories.replace(fats, ""),
                    taxa.into(),
                    rules.replace(oil, ""),
                    exceptions.into(),
                ],
                "diet/categories.toml has no class for category Fats",
            ),
            (
                [
                    categories.replace(fats, ""),
                    taxa.into(),
                    rules.into(),
                    exceptions.into(),
                ],
                "diet/rules.toml Fats / \"Oil,\": Constraint Error: Violates foreign key",
            ),
            (
                [
                    format!(
                        "{categories}[[category]]\ndescription = \"Seafood\"\nclass = \"flesh\"\n"
                    ),
                    taxa.into(),
                    rules.into(),
                    exceptions.into(),
                ],
                "diet/categories.toml: category Seafood is in no dataset",
            ),
            (
                [
                    categories.into(),
                    taxa.replace(animal, ""),
                    rules.into(),
                    exceptions.into(),
                ],
                "diet/taxa.toml lacks NCBITaxon_33208, which food",
            ),
            (
                [
                    categories.into(),
                    taxa.into(),
                    format!(
                        "{rules}[[rule]]\ncategory = \"Dairy\"\nstarts_with = \"Butter\"\nvegan = \"no\"\nwhy = \"x\"\n"
                    ),
                    exceptions.into(),
                ],
                "diet/rules.toml: no food in Dairy starts with Butter",
            ),
            (
                [
                    categories.into(),
                    taxa.into(),
                    format!(
                        "{rules}[[rule]]\ncategory = \"Dairy\"\nstarts_with = \"Cheese, parm\"\nvegan = \"no\"\nwhy = \"x\"\n"
                    ),
                    exceptions.into(),
                ],
                "diet/rules.toml: food 4 matches two rules: Cheese and Cheese, parm",
            ),
            (
                [
                    categories.into(),
                    taxa.into(),
                    rules.into(),
                    exceptions
                        .replace("description = \"Lard\"", "description = \"Lard, rendered\""),
                ],
                "diet/exceptions.toml: food 7 is Lard in USDA's data, not Lard, rendered",
            ),
            (
                [
                    categories.into(),
                    taxa.into(),
                    rules.into(),
                    format!(
                        "{exceptions}[[food]]\nfdc_id = 3\ndescription = \"C\"\nvegan = \"no\"\nwhy = \"x\"\n"
                    ),
                ],
                "diet/exceptions.toml 3:",
            ),
        ];
        for (diet, why) in cases {
            let texts = [
                diet[0].as_str(),
                diet[1].as_str(),
                diet[2].as_str(),
                diet[3].as_str(),
            ];
            let Err(err) = synthetic_with(SYNTHETIC_NUTRIENTS, texts) else {
                panic!("loaded, though it should refuse: {why}");
            };
            assert!(err.contains(why), "{err:?} does not say {why:?}");
        }
    }

    /// A form's FoodOn id comes from USDA's data for its food, once however often USDA repeats it;
    /// a food USDA gives none has none. Candidates are the names a linked food's common name gives,
    /// less those an ingredient already lists ("a-food" and "ay" are aliases; "plain a" is not).
    #[test]
    fn foodon_ids_are_derived_and_candidates_are_names_not_yet_aliases() {
        let w = synthetic(SYNTHETIC_NUTRIENTS).unwrap();
        assert_eq!(
            strings(
                &w,
                "SELECT ingredient || ' ' || state || ' ' || coalesce(foodon, '-') || ' ' || is_default FROM gold.ingredient_form ORDER BY ingredient"
            ),
            [
                "a-food raw - true",
                "olive-oil as_sold FOODON_03301826 true"
            ]
        );
        assert_eq!(
            strings(
                &w,
                "SELECT ingredient || ': ' || candidate || ' (' || source || ')' FROM gold.alias_candidate ORDER BY 1"
            ),
            ["a-food: plain a (SR Legacy common name)"]
        );
    }

    /// The report counts the nutrients `nutrients.toml` lists rather than naming a number: the
    /// synthetic warehouse lists two.
    #[test]
    fn the_ingredients_report_counts_the_nutrients_listed() {
        let w = synthetic(SYNTHETIC_NUTRIENTS).unwrap();
        let report = ingredients_report(&w).unwrap();
        assert!(
            report.contains("| of 2 |") && report.contains("\"Of\n2\" is how many"),
            "{report}"
        );
    }

    /// Olive oil's organism is read from food 6's NCBI Taxon, which USDA repeats, and a-food's from
    /// Wikidata item Q1 through its P1582, Q2. Each Foundation food's own organism is in silver once,
    /// a food recorded as NA has none, and each pinned item's statements of the properties read are
    /// there.
    #[test]
    fn each_ingredient_s_organism_is_read_from_its_source() {
        let w = synthetic(SYNTHETIC_NUTRIENTS).unwrap();
        assert_eq!(
            strings(
                &w,
                "SELECT ingredient || ' ' || taxon || ' ' || coalesce(fdc_id::VARCHAR, '-') || ' '
                        || coalesce(wikidata, '-') || ' ' || coalesce(taxon_item, '-') || ' ' || coalesce(none, '-')
                 FROM gold.ingredient_organism ORDER BY ingredient"
            ),
            [
                "a-food NCBITaxon_111 - Q1 Q2 -",
                "olive-oil NCBITaxon_4146 6 - - -"
            ]
        );
        assert_eq!(
            strings(
                &w,
                "SELECT fdc_id || ' ' || taxon FROM silver.usda_food_organism ORDER BY fdc_id"
            ),
            ["2 NCBITaxon_9913", "6 NCBITaxon_4146"]
        );
        assert_eq!(
            strings(
                &w,
                "SELECT item || ' ' || property || ' ' || position || ' ' || value || ' ' || rank
                 FROM silver.wikidata_statement ORDER BY item, property, position"
            ),
            [
                "Q1 P1582 1 Q2 normal",
                "Q2 P225 1 A b normal",
                "Q2 P685 1 111 normal"
            ]
        );
        let broth = SYNTHETIC_INGREDIENTS[0]
            .1
            .replace(
                "[organism]\ntaxon = \"NCBITaxon_111\"\nwikidata = \"Q1\"\n",
                "[organism]\nnone = \"A mix.\"\n",
            )
            .replace("harvested = true", "none = \"Of several.\"");
        let w = synthetic_pinned(
            SYNTHETIC_NUTRIENTS,
            SYNTHETIC_DIET,
            &[("a-food.toml", &broth), SYNTHETIC_INGREDIENTS[1]],
            SYNTHETIC_COUNT_UNITS,
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(
            strings(
                &w,
                "SELECT ingredient || ' ' || coalesce(taxon, '-') || ' ' || coalesce(none, '-')
                 FROM gold.ingredient_organism ORDER BY ingredient"
            ),
            ["a-food - A mix.", "olive-oil NCBITaxon_4146 -"]
        );
    }

    /// Each ingredient's origin is in gold as its file gives it, with what it is made from in the
    /// file's order. Silver's keys refuse an origin value they do not know, a reason without
    /// `none` or `none` without one, a blank reason, an origin or a source that is no ingredient, a
    /// second origin for one ingredient, a source listed twice or two at one position, a position
    /// below 1, an ingredient made directly from itself, and a source for an ingredient with no
    /// origin; the records' other origin rules, a longer way back among them, silver does not
    /// hold.
    #[test]
    fn each_ingredient_s_origin_is_loaded_and_its_keys_hold() {
        let juice = SYNTHETIC_INGREDIENTS[0]
            .1
            .replace("[\"a-food\", \"ay\"]", "[\"a juice\"]")
            .replace("harvested = true", "made_from = [\"a-food\"]");
        let w = synthetic_pinned(
            SYNTHETIC_NUTRIENTS,
            SYNTHETIC_DIET,
            &[
                SYNTHETIC_INGREDIENTS[0],
                ("a-juice.toml", &juice),
                SYNTHETIC_INGREDIENTS[1],
            ],
            SYNTHETIC_COUNT_UNITS,
            &synthetic_wikidata(),
            &[],
        )
        .unwrap();
        assert_eq!(
            strings(
                &w,
                "SELECT ingredient || ' ' || origin || ' ' || coalesce(none, '-')
                 FROM gold.ingredient_origin ORDER BY ingredient"
            ),
            [
                "a-food harvested -",
                "a-juice made_from -",
                "olive-oil none Pressed from olives."
            ]
        );
        assert_eq!(
            strings(
                &w,
                "SELECT ingredient || ' ' || position || ' ' || made_from
                 FROM gold.ingredient_made_from ORDER BY ingredient, position"
            ),
            ["a-juice 1 a-food"]
        );
        let accepted = |sql: &str| {
            w.execute_batch(&format!(
                "BEGIN; INSERT INTO silver.ingredient VALUES ('b', 'B', NULL, 'x'); {sql}; ROLLBACK;"
            ))
            .map_err(|e| {
                let _ = w.execute_batch("ROLLBACK");
                e.to_string()
            })
        };
        let cases = [
            (
                "INSERT INTO silver.ingredient_origin VALUES ('b', 'harvested', NULL)",
                true,
            ),
            (
                "INSERT INTO silver.ingredient_origin VALUES ('b', 'grown', NULL)",
                false,
            ),
            (
                "INSERT INTO silver.ingredient_origin VALUES ('b', 'harvested', 'Why.')",
                false,
            ),
            (
                "INSERT INTO silver.ingredient_origin VALUES ('b', 'none', NULL)",
                false,
            ),
            (
                "INSERT INTO silver.ingredient_origin VALUES ('b', 'none', ' ')",
                false,
            ),
            (
                "INSERT INTO silver.ingredient_origin VALUES ('c', 'harvested', NULL)",
                false,
            ),
            (
                "INSERT INTO silver.ingredient_origin VALUES ('a-food', 'harvested', NULL)",
                false,
            ),
            (
                "INSERT INTO silver.ingredient_made_from VALUES ('a-juice', 2, 'b')",
                true,
            ),
            (
                "INSERT INTO silver.ingredient_made_from VALUES ('a-juice', 2, 'a-food')",
                false,
            ),
            (
                "INSERT INTO silver.ingredient_made_from VALUES ('a-juice', 1, 'b')",
                false,
            ),
            (
                "INSERT INTO silver.ingredient_made_from VALUES ('a-juice', 0, 'b')",
                false,
            ),
            (
                "INSERT INTO silver.ingredient_made_from VALUES ('a-juice', 2, 'c')",
                false,
            ),
            (
                "INSERT INTO silver.ingredient_made_from VALUES ('a-food', 1, 'a-food')",
                false,
            ),
            (
                "INSERT INTO silver.ingredient_made_from VALUES ('b', 1, 'a-food')",
                false,
            ),
        ];
        for (sql, ok) in cases {
            assert_eq!(accepted(sql).is_ok(), ok, "{sql}: {:?}", accepted(sql));
        }
    }

    /// The load stops on an organism its source does not give: a food named by another name than
    /// USDA's, one USDA's data records no NCBI Taxon for (an SR Legacy food, or NA), one whose NCBI
    /// Taxon is another, a food not in the pinned release, a Wikidata item that gives another taxon
    /// or is not pinned, a pinned item no organism reads, and pinned content that is another item's.
    #[test]
    fn the_load_refuses_an_organism_its_source_does_not_give() {
        let oil = SYNTHETIC_INGREDIENTS[1].1;
        let a = SYNTHETIC_INGREDIENTS[0].1;
        let oil_with = |old: &str, new: &str| {
            let organism = &oil[oil.find("[organism]").unwrap()..oil.find("[[form]]").unwrap()];
            assert_eq!(organism.matches(old).count(), 1, "{old:?}");
            oil.replace(organism, &organism.replace(old, new))
        };
        let pinned = synthetic_wikidata();
        let mut unread = pinned.clone();
        unread.push(("Q3".into(), wikidata_item("Q3", &[("P685", "3", "normal")])));
        let mut swapped = pinned.clone();
        swapped[1].1 = wikidata_item("Q5", &[("P685", "111", "normal")]);
        let cases = [
            (
                a.into(),
                oil_with(
                    "description = \"Oil, olive\"",
                    "description = \"Oil, olives\"",
                ),
                &pinned,
                "ingredients/olive-oil.toml: organism: food 6 is Oil, olive in USDA's data, not Oil, olives",
            ),
            (
                a.into(),
                oil_with(
                    "fdc_id = 6\ndescription = \"Oil, olive\"",
                    "fdc_id = 1\ndescription = \"A\"",
                ),
                &pinned,
                "ingredients/olive-oil.toml: organism: food 1 records no NCBI Taxon in USDA's data",
            ),
            (
                a.into(),
                oil_with(
                    "fdc_id = 6\ndescription = \"Oil, olive\"",
                    "fdc_id = 10\ndescription = \"D\"",
                ),
                &pinned,
                "ingredients/olive-oil.toml: organism: food 10 records no NCBI Taxon in USDA's data",
            ),
            (
                a.into(),
                oil_with("NCBITaxon_4146", "NCBITaxon_4147"),
                &pinned,
                "ingredients/olive-oil.toml: organism: food 6 records NCBITaxon_4146 in USDA's data, not NCBITaxon_4147",
            ),
            (
                a.into(),
                oil_with("fdc_id = 6", "fdc_id = 99"),
                &pinned,
                "ingredients/olive-oil.toml organism: Constraint Error: Violates foreign key",
            ),
            (
                a.replace("NCBITaxon_111", "NCBITaxon_112"),
                oil.into(),
                &pinned,
                "ingredients/a-food.toml: organism: Wikidata gives NCBITaxon_111, not NCBITaxon_112",
            ),
            (
                a.replace("wikidata = \"Q1\"", "wikidata = \"Q9\""),
                oil.into(),
                &pinned,
                "ingredients/a-food.toml: organism: Wikidata item Q9 is not pinned in sources.toml",
            ),
            (
                a.into(),
                oil.into(),
                &unread,
                "sources.toml: Wikidata item Q3 is pinned, but no ingredient's or crop's organism reads it",
            ),
            (
                a.into(),
                oil.into(),
                &swapped,
                "wikidata Q2: the content is \"Q5\"'s",
            ),
        ];
        for (a_food, olive_oil, wikidata, why) in cases {
            let files = [
                ("a-food.toml", a_food.as_str()),
                ("olive-oil.toml", olive_oil.as_str()),
            ];
            let Err(err) = synthetic_pinned(
                SYNTHETIC_NUTRIENTS,
                SYNTHETIC_DIET,
                &files,
                SYNTHETIC_COUNT_UNITS,
                wikidata,
                &[],
            ) else {
                panic!("loaded, though it should refuse: {why}");
            };
            assert!(err.contains(why), "{err:?} does not say {why:?}");
        }
    }

    /// a-plant's organism is a-food's, read from its own item; a-variety's is one step below it,
    /// read from Q3, whose parent taxon Q10 gives a-food's organism.
    #[test]
    fn each_crop_s_organism_is_its_ingredient_s_or_one_step_below() {
        let w = synthetic_crops(
            &SYNTHETIC_INGREDIENTS,
            &crop_wikidata(&[]),
            &SYNTHETIC_CROPS,
        )
        .unwrap();
        assert_eq!(
            strings(
                &w,
                "SELECT crop || ' ' || ingredient || ' ' || state || ' ' || taxon || ' ' || scientific_name
                        || ' ' || wikidata || ' ' || coalesce(parent_item, '-') || ' ' || steps_below
                 FROM gold.crop ORDER BY crop"
            ),
            [
                "a-plant a-food raw NCBITaxon_111 A b Q2 - 0",
                "a-variety a-food raw NCBITaxon_112 A b var. c Q3 Q10 1"
            ]
        );
    }

    /// The load stops on a crop whose item is not pinned, gives another taxon or name, or lacks
    /// either; whose ingredient is not there, has no organism, is not harvested, or has no form in
    /// the crop's state;
    /// and whose organism is not its ingredient's and not one step below it: a parent taxon with
    /// another organism, two steps down, none, or one not pinned.
    #[test]
    fn the_load_refuses_a_crop_its_organism_does_not_fit() {
        let [plant, variety] = SYNTHETIC_CROPS.map(|(_, t)| t);
        let in_plant = |old: &str, new: &str| {
            assert_eq!(plant.matches(old).count(), 1, "{old:?}");
            plant.replace(old, new)
        };
        let a_food_none = SYNTHETIC_INGREDIENTS[0]
            .1
            .replace(
                "[organism]\ntaxon = \"NCBITaxon_111\"\nwikidata = \"Q1\"\n",
                "[organism]\nnone = \"A mix.\"\n",
            )
            .replace("harvested = true", "none = \"Of several.\"");
        let none_ingredients = [
            ("a-food.toml", a_food_none.as_str()),
            SYNTHETIC_INGREDIENTS[1],
        ];
        let oil_harvested = SYNTHETIC_INGREDIENTS[1]
            .1
            .replace("none = \"Pressed from olives.\"", "harvested = true");
        let harvested_ingredients = [
            SYNTHETIC_INGREDIENTS[0],
            ("olive-oil.toml", oil_harvested.as_str()),
        ];
        let crop = |item: &str, taxon: &str, name: &str, ingredient: &str, state: &str| {
            format!(
                "name = \"C\"\nwikidata = \"{item}\"\ntaxon = \"{taxon}\"\nscientific_name = \"{name}\"\ningredient = \"{ingredient}\"\nstate = \"{state}\"\n"
            )
        };
        let cases: Vec<CropCase> = vec![
            (
                &SYNTHETIC_INGREDIENTS,
                crop_wikidata(&[]),
                in_plant("\"Q2\"", "\"Q9\""),
                "crops/c.toml: Wikidata item Q9 is not pinned in sources.toml",
            ),
            (
                &SYNTHETIC_INGREDIENTS,
                crop_wikidata(&[]),
                in_plant("NCBITaxon_111", "NCBITaxon_113"),
                "crops/c.toml: Wikidata gives NCBITaxon_111, not NCBITaxon_113",
            ),
            (
                &SYNTHETIC_INGREDIENTS,
                crop_wikidata(&[]),
                in_plant("\"A b\"", "\"A c\""),
                "crops/c.toml: Wikidata item Q2 names NCBITaxon_111 A b, not A c",
            ),
            (
                &SYNTHETIC_INGREDIENTS,
                crop_wikidata(&[("Q4", &[("P685", "111", "normal")])]),
                crop("Q4", "NCBITaxon_111", "A b", "a-food", "raw"),
                "crops/c.toml: Wikidata item Q4 has no P225",
            ),
            (
                &SYNTHETIC_INGREDIENTS,
                crop_wikidata(&[]),
                crop("Q1", "NCBITaxon_111", "A b", "a-food", "raw"),
                "crops/c.toml: Wikidata item Q1 has no P685",
            ),
            (
                &SYNTHETIC_INGREDIENTS,
                crop_wikidata(&[]),
                in_plant("\"a-food\"", "\"b-food\""),
                "crops/c.toml: ingredient b-food is not in ingredients/",
            ),
            (
                &none_ingredients,
                crop_wikidata(&[]),
                plant.to_string(),
                "crops/c.toml: ingredient a-food has no organism, so no crop yields it",
            ),
            (
                &SYNTHETIC_INGREDIENTS,
                crop_wikidata(&[]),
                in_plant("\"a-food\"", "\"olive-oil\""),
                "crops/c.toml: ingredient olive-oil is not harvested, so no crop yields it",
            ),
            (
                &SYNTHETIC_INGREDIENTS,
                crop_wikidata(&[]),
                in_plant("\"raw\"", "\"dry\""),
                "crops/c.toml: Constraint Error: Violates foreign key",
            ),
            (
                &harvested_ingredients,
                crop_wikidata(&[]),
                variety.replace("\"a-food\"", "\"olive-oil\""),
                "crops/c.toml: NCBITaxon_112 is neither olive-oil's organism, NCBITaxon_4146, nor one step below it: Wikidata item Q3's P171 is Q10, NCBITaxon_111",
            ),
            (
                &SYNTHETIC_INGREDIENTS,
                crop_wikidata(&[(
                    "Q5",
                    &[
                        ("P685", "113", "normal"),
                        ("P225", "A b var. c f. d", "normal"),
                        ("P171", "Q3", "normal"),
                    ],
                )]),
                crop("Q5", "NCBITaxon_113", "A b var. c f. d", "a-food", "raw"),
                "crops/c.toml: NCBITaxon_113 is neither a-food's organism, NCBITaxon_111, nor one step below it: Wikidata item Q5's P171 is Q3, NCBITaxon_112",
            ),
            (
                &SYNTHETIC_INGREDIENTS,
                crop_wikidata(&[("Q6", &[("P685", "114", "normal"), ("P225", "X", "normal")])]),
                crop("Q6", "NCBITaxon_114", "X", "a-food", "raw"),
                "crops/c.toml: Wikidata item Q6 has no P171",
            ),
            (
                &SYNTHETIC_INGREDIENTS,
                crop_wikidata(&[(
                    "Q7",
                    &[
                        ("P685", "115", "normal"),
                        ("P225", "Y", "normal"),
                        ("P171", "Q99", "normal"),
                    ],
                )]),
                crop("Q7", "NCBITaxon_115", "Y", "a-food", "raw"),
                "crops/c.toml: Wikidata item Q7's P171 is Q99, which sources.toml does not pin",
            ),
        ];
        for (ingredients, wikidata, text, why) in cases {
            let Err(err) = synthetic_crops(ingredients, &wikidata, &[("c.toml", &text)]) else {
                panic!("loaded, though it should refuse: {why}");
            };
            assert!(err.contains(why), "{err:?} does not say {why:?}");
        }
        let only_plant = synthetic_crops(
            &SYNTHETIC_INGREDIENTS,
            &crop_wikidata(&[]),
            &SYNTHETIC_CROPS[..1],
        );
        let why = "sources.toml: Wikidata item Q10 is pinned, but no ingredient's or crop's organism reads it";
        assert!(
            only_plant.as_ref().err().is_some_and(|e| e.contains(why)),
            "{:?}",
            only_plant.err()
        );
    }

    #[test]
    fn the_load_refuses_a_form_that_is_not_usda_s() {
        let unknown_food = SYNTHETIC_INGREDIENTS[0]
            .1
            .replace("fdc_id = 1", "fdc_id = 3");
        let renamed = SYNTHETIC_INGREDIENTS[0]
            .1
            .replace("description = \"A\"", "description = \"Ay\"");
        let cases = [
            (
                unknown_food,
                "ingredients/a-food.toml raw 3: Constraint Error: Violates foreign key",
            ),
            (
                renamed,
                "ingredients/a-food.toml: food 1 is A in USDA's data, not Ay",
            ),
        ];
        for (text, why) in cases {
            let files = [("a-food.toml", text.as_str()), SYNTHETIC_INGREDIENTS[1]];
            let Err(err) = synthetic_full(
                SYNTHETIC_NUTRIENTS,
                SYNTHETIC_DIET,
                &files,
                SYNTHETIC_COUNT_UNITS,
            ) else {
                panic!("loaded, though it should refuse: {why}");
            };
            assert!(err.contains(why), "{err:?} does not say {why:?}");
        }
    }

    /// The warehouse, loaded once from the repo's records and the cached zips.
    fn warehouse() -> MutexGuard<'static, Connection> {
        static WAREHOUSE: OnceLock<Mutex<Connection>> = OnceLock::new();
        WAREHOUSE
            .get_or_init(|| {
                let cache = cache_dir().unwrap();
                Mutex::new(load(&repo_root(), &cache).unwrap_or_else(|e| panic!("{e}")))
            })
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn count(conn: &Connection, query: &str) -> i64 {
        conn.query_row(query, [], |r| r.get(0))
            .unwrap_or_else(|e| panic!("{query}: {e}"))
    }

    #[test]
    fn the_sources_record_is_in_bronze_as_read() {
        let w = warehouse();
        let body: String = w
            .query_row(
                "SELECT body FROM bronze.record WHERE path = 'sources.toml'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            body,
            fs::read_to_string(repo_root().join("sources.toml")).unwrap()
        );
    }

    /// The Done of plan step 3: SR Legacy ships its own count of each table's rows.
    #[test]
    fn the_sr_legacy_row_counts_equal_usda_s_own() {
        let w = warehouse();
        let mut stmt = w
            .prepare(
                "SELECT \"Table\", \"Number of Records\" FROM bronze.usda_all_downloaded_table_record_counts WHERE dataset = 'sr-legacy'",
            )
            .unwrap();
        let counts: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(counts.len() >= 9, "{counts:?}");
        for (table, n) in counts {
            let got = count(
                &w,
                &format!("SELECT count(*) FROM bronze.usda_{table} WHERE dataset = 'sr-legacy'"),
            );
            assert_eq!(got.to_string(), n, "bronze.usda_{table}");
        }
    }

    /// The number of records in a CSV, header excluded, counted apart from DuckDB: a line break
    /// inside double quotes does not end a record, and a final line without a break still counts.
    fn csv_records(bytes: &[u8]) -> i64 {
        let (mut lines, mut quoted) = (0, false);
        for &b in bytes {
            match b {
                b'"' => quoted = !quoted,
                b'\n' if !quoted => lines += 1,
                _ => {}
            }
        }
        if bytes.last().is_some_and(|&b| b != b'\n') {
            lines += 1;
        }
        lines - 1
    }

    #[test]
    fn what_counts_as_a_csv_record() {
        assert_eq!(csv_records(b"a,b\n1,2\n3,4\n"), 2);
        assert_eq!(csv_records(b"a,b\n1,2\n3,4"), 2);
        assert_eq!(csv_records(b"a,b\n\"1\n1\",2\n"), 1);
        assert_eq!(csv_records(b"a,b\n\"say \"\"hi\"\"\",2\n"), 1);
        assert_eq!(csv_records(b"a,b\n"), 0);
    }

    #[test]
    fn every_csv_in_the_pinned_zips_is_in_bronze_row_for_row() {
        let w = warehouse();
        let (sources, _) = Sources::load(&repo_root()).unwrap();
        let cache = cache_dir().unwrap();
        let scratch = Scratch::new().unwrap();
        let mut csvs = 0;
        for d in &sources.dataset {
            for m in usda::extract(&cache, d, &scratch.0).unwrap() {
                let loaded_as: Option<String> = w
                    .query_row(
                        "SELECT loaded_as FROM bronze.usda_member WHERE dataset = ? AND member = ?",
                        params![d.id, m.name],
                        |r| r.get(0),
                    )
                    .unwrap_or_else(|e| {
                        panic!("{} {}: not in bronze.usda_member: {e}", d.id, m.name)
                    });
                let Some(table) = loaded_as else {
                    assert!(
                        !m.name.ends_with(".csv"),
                        "{} {} was not loaded",
                        d.id,
                        m.name
                    );
                    continue;
                };
                csvs += 1;
                let rows = count(
                    &w,
                    &format!(
                        "SELECT count(*) FROM bronze.{table} WHERE dataset = {}",
                        literal(&d.id)
                    ),
                );
                assert_eq!(
                    rows,
                    csv_records(&fs::read(&m.file).unwrap()),
                    "{} {}: bronze.{table} has {rows} rows",
                    d.id,
                    m.name
                );
            }
        }
        assert!(csvs >= 30, "found {csvs} CSVs");
    }

    /// bronze.usda_member gives each file the length and sha256 of the bytes extracted from the
    /// pinned zip, not some other file's.
    #[test]
    fn every_file_in_the_pinned_zips_is_in_bronze_with_its_sha256() {
        let w = warehouse();
        let (sources, _) = Sources::load(&repo_root()).unwrap();
        let cache = cache_dir().unwrap();
        let scratch = Scratch::new().unwrap();
        let mut files = 0;
        for d in &sources.dataset {
            for m in usda::extract(&cache, d, &scratch.0).unwrap() {
                let (bytes, sha256): (i64, String) = w
                    .query_row(
                        "SELECT bytes, sha256 FROM bronze.usda_member WHERE dataset = ? AND member = ?",
                        params![d.id, m.name],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .unwrap_or_else(|e| panic!("{} {}: {e}", d.id, m.name));
                let content = fs::read(&m.file).unwrap();
                assert_eq!(bytes as usize, content.len(), "{} {}", d.id, m.name);
                assert_eq!(sha256, usda::sha256_hex(&content), "{} {}", d.id, m.name);
                files += 1;
            }
            let listed = count(
                &w,
                &format!(
                    "SELECT count(*) FROM bronze.usda_member WHERE dataset = {}",
                    literal(&d.id)
                ),
            );
            assert_eq!(
                listed as usize, files,
                "{}: files listed but not in its zip",
                d.id
            );
            files = 0;
        }
    }

    #[test]
    fn silver_holds_every_sr_legacy_and_foundation_food() {
        let w = warehouse();
        let expected = count(
            &w,
            "SELECT count(*) FROM bronze.usda_food WHERE data_type IN ('sr_legacy_food', 'foundation_food')",
        );
        assert!(expected >= 8000, "{expected}");
        assert_eq!(count(&w, "SELECT count(*) FROM silver.usda_food"), expected);
        assert_eq!(
            count(
                &w,
                "SELECT count(*) FROM silver.usda_food s JOIN bronze.usda_food b \
                 ON CAST(b.fdc_id AS INTEGER) = s.fdc_id AND b.dataset = s.dataset \
                 AND b.description = s.description AND b.data_type = s.data_type"
            ),
            expected
        );
    }

    /// Every food_nutrient row of a silver food is kept or rejected, never both and never neither.
    #[test]
    fn every_food_nutrient_row_in_scope_is_kept_or_rejected() {
        let w = warehouse();
        let in_scope = "SELECT dataset, id FROM bronze.usda_food_nutrient \
                        WHERE CAST(fdc_id AS INTEGER) IN (SELECT fdc_id FROM silver.usda_food)";
        let kept = "SELECT dataset, source_id FROM silver.usda_food_nutrient";
        let rejected =
            "SELECT dataset, id FROM silver.usda_rejected WHERE source = 'usda_food_nutrient'";
        let n = |q: &str| count(&w, &format!("SELECT count(*) FROM ({q})"));
        assert!(n(in_scope) >= 600_000);
        assert_eq!(n(&format!("{kept} UNION ALL {rejected}")), n(in_scope));
        assert_eq!(
            n(&format!("{in_scope} EXCEPT ({kept} UNION {rejected})")),
            0
        );
        assert_eq!(n(&format!("{kept} INTERSECT {rejected}")), 0);
    }

    /// One organism can be several crops, so an organism does not say which ingredient a crop is:
    /// USDA's data gives its Foundation broccoli, kale and collards one taxon, Brassica oleracea.
    #[test]
    fn one_organism_can_be_several_crops() {
        let w = warehouse();
        assert_eq!(
            strings(
                &w,
                "SELECT DISTINCT split_part(f.description, ',', 1) || ' ' || o.taxon
                 FROM silver.usda_food_organism o JOIN silver.usda_food f USING (fdc_id)
                 WHERE split_part(f.description, ',', 1) IN ('Broccoli', 'Kale', 'Collards')
                 ORDER BY 1"
            ),
            [
                "Broccoli NCBITaxon_3712",
                "Collards NCBITaxon_3712",
                "Kale NCBITaxon_3712"
            ]
        );
    }

    /// The crops read their organisms from their pinned items: carrot and cherry tomato one step
    /// below their ingredient's species, through its item; the rest that species itself. Tomato
    /// and cherry tomato both yield the tomato ingredient.
    #[test]
    fn the_crops_read_their_organisms_from_their_items() {
        let w = warehouse();
        assert!(count(&w, "SELECT count(*) FROM gold.crop") >= 10);
        for (crop, row) in [
            ("carrot", "carrot raw NCBITaxon_79200 Q11678009 Q30056 1"),
            (
                "cherry-tomato",
                "tomato raw NCBITaxon_195583 Q6540634 Q23501 1",
            ),
            ("tomato", "tomato raw NCBITaxon_4081 Q23501 - 0"),
            ("chickpea", "chickpeas dry NCBITaxon_3827 Q81375 - 0"),
            ("cumin", "cumin dry NCBITaxon_52462 Q132624 - 0"),
        ] {
            let got = strings(
                &w,
                &format!(
                    "SELECT ingredient || ' ' || state || ' ' || taxon || ' ' || wikidata || ' '
                            || coalesce(parent_item, '-') || ' ' || steps_below
                     FROM gold.crop WHERE crop = {}",
                    literal(crop)
                ),
            );
            assert_eq!(got, [row], "{crop}");
        }
    }

    /// Every record file the warehouse loads lies in a path `DATA-LICENSE.md` lists as CC0: a file
    /// listed by name, or one inside a folder listed with its trailing slash.
    #[test]
    fn every_record_the_warehouse_loads_is_under_the_data_license() {
        let w = warehouse();
        let license = fs::read_to_string(repo_root().join("DATA-LICENSE.md")).unwrap();
        let (_, listed) = license.split_once("## What is CC0").unwrap();
        let listed = &listed[..listed.find("\n## ").unwrap()];
        let paths: Vec<&str> = listed
            .lines()
            .filter_map(|l| l.strip_prefix("- `")?.strip_suffix('`'))
            .collect();
        assert!(
            paths.contains(&"crops/") && paths.contains(&"sources.toml"),
            "{paths:?}"
        );
        let records = strings(&w, "SELECT path FROM bronze.record ORDER BY path");
        assert!(records.len() >= 30, "{records:?}");
        for record in &records {
            assert!(
                paths
                    .iter()
                    .any(|p| record == p || (p.ends_with('/') && record.starts_with(p))),
                "{record} lies in no path DATA-LICENSE.md lists"
            );
        }
    }

    /// bronze.wikidata_revision holds each item `sources.toml` pins, at its revision, with content
    /// whose length and sha256 are the pin's, and no other item.
    #[test]
    fn every_pinned_wikidata_revision_is_in_bronze_as_pinned() {
        let w = warehouse();
        let (sources, _) = Sources::load(&repo_root()).unwrap();
        assert!(sources.wikidata.len() >= 2);
        for pin in &sources.wikidata {
            let (revision, sha256, bytes, content): (i64, String, i64, String) = w
                .query_row(
                    "SELECT revision, sha256, bytes, content FROM bronze.wikidata_revision WHERE item = ?",
                    params![pin.item],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap_or_else(|e| panic!("{}: {e}", pin.item));
            assert_eq!(
                (revision as u64, sha256.as_str(), bytes as u64),
                (pin.revision, pin.sha256.as_str(), pin.bytes),
                "{}",
                pin.item
            );
            assert_eq!(content.len() as u64, pin.bytes, "{}", pin.item);
            assert_eq!(
                usda::sha256_hex(content.as_bytes()),
                pin.sha256,
                "{}",
                pin.item
            );
        }
        assert_eq!(
            count(&w, "SELECT count(*) FROM bronze.wikidata_revision") as usize,
            sources.wikidata.len()
        );
    }

    /// The red lentil soup's ingredients read their organisms from the pinned data: lemon juice and
    /// cumin from Wikidata through a natural product of taxon, red lentils from lentils' Foundation
    /// food, parmesan from its cow's, and vegetable broth has none. Every ingredient has a row.
    #[test]
    fn the_red_lentil_soup_s_organisms_are_read_from_their_sources() {
        let w = warehouse();
        assert_eq!(
            count(&w, "SELECT count(*) FROM gold.ingredient_organism"),
            count(&w, "SELECT count(*) FROM silver.ingredient")
        );
        for (ingredient, row) in [
            ("red-lentils", "NCBITaxon_3864 2644283 - -"),
            ("onion", "NCBITaxon_4679 790646 - -"),
            ("garlic", "NCBITaxon_4682 1104647 - -"),
            ("olive-oil", "NCBITaxon_4146 748608 - -"),
            ("parmesan", "NCBITaxon_9913 325036 - -"),
            ("cumin", "NCBITaxon_52462 - Q57328174 Q132624"),
            ("lemon-juice", "NCBITaxon_2708 - Q1375049 Q500"),
            ("vegetable-broth", "- - - -"),
        ] {
            let got = strings(
                &w,
                &format!(
                    "SELECT coalesce(taxon, '-') || ' ' || coalesce(fdc_id::VARCHAR, '-') || ' '
                            || coalesce(wikidata, '-') || ' ' || coalesce(taxon_item, '-')
                     FROM gold.ingredient_organism WHERE ingredient = {}",
                    literal(ingredient)
                ),
            );
            assert_eq!(got, [row], "{ingredient}");
        }
    }

    /// A silver row names a dataset bronze.usda_zip holds, or a record bronze.record holds: the
    /// link DuckDB will not hold as a foreign key across schemas. A Wikidata statement names an
    /// item bronze.wikidata_revision holds.
    #[test]
    fn every_silver_row_traces_to_the_bytes_it_came_from() {
        let w = warehouse();
        for table in [
            "usda_category",
            "usda_food",
            "usda_nutrient",
            "usda_food_nutrient",
            "usda_rejected",
        ] {
            let untraced = count(
                &w,
                &format!(
                    "SELECT count(*) FROM silver.{table} WHERE dataset NOT IN (SELECT dataset FROM bronze.usda_zip)"
                ),
            );
            assert_eq!(untraced, 0, "silver.{table}");
        }
        assert_eq!(
            count(
                &w,
                "SELECT count(*) FROM silver.usda_food_nutrient s WHERE NOT EXISTS \
                 (SELECT 1 FROM bronze.usda_food_nutrient b WHERE b.dataset = s.dataset AND b.id = s.source_id)"
            ),
            0
        );
        assert_eq!(
            count(
                &w,
                "SELECT count(*) FROM silver.usda_portion s WHERE NOT EXISTS \
                 (SELECT 1 FROM bronze.usda_food_portion b WHERE b.dataset = s.dataset AND b.id = s.id)"
            ),
            0
        );
        assert_eq!(
            count(
                &w,
                "SELECT count(*) FROM silver.nutrient WHERE record NOT IN (SELECT path FROM bronze.record)"
            ),
            0
        );
        assert!(count(&w, "SELECT count(*) FROM silver.nutrient") >= 16);
        for table in [
            "diet_category",
            "diet_taxon",
            "diet_rule",
            "diet_exception",
            "ingredient",
            "crop",
            "count_unit",
        ] {
            let untraced = count(
                &w,
                &format!(
                    "SELECT count(*) FROM silver.{table} WHERE record NOT IN (SELECT path FROM bronze.record)"
                ),
            );
            assert_eq!(untraced, 0, "silver.{table}");
        }
        assert_eq!(
            count(
                &w,
                "SELECT count(*) FROM silver.wikidata_statement WHERE item NOT IN (SELECT item FROM bronze.wikidata_revision)"
            ),
            0
        );
        assert!(count(&w, "SELECT count(*) FROM silver.wikidata_statement") >= 2);
        for table in [
            "usda_food_taxon",
            "usda_food_organism",
            "usda_food_foodon",
            "usda_food_common_name",
            "usda_measure_unit",
            "usda_portion",
            "usda_portion_unit",
        ] {
            let untraced = count(
                &w,
                &format!(
                    "SELECT count(*) FROM silver.{table} WHERE dataset NOT IN (SELECT dataset FROM bronze.usda_zip)"
                ),
            );
            assert_eq!(untraced, 0, "silver.{table}");
        }
    }

    /// The Done of plan step 7: every portion of a silver food is kept or rejected, and every kept
    /// portion has its unit read or is listed in docs/portions.md by its leading phrase.
    #[test]
    fn every_portion_is_read_or_listed() {
        let w = warehouse();
        let in_scope = count(
            &w,
            "SELECT count(*) FROM bronze.usda_food_portion WHERE CAST(fdc_id AS INTEGER) IN (SELECT fdc_id FROM silver.usda_food)",
        );
        let kept = count(&w, "SELECT count(*) FROM silver.usda_portion");
        let rejected = count(
            &w,
            "SELECT count(*) FROM silver.usda_rejected WHERE source = 'usda_food_portion'",
        );
        assert!(kept >= 14_000, "{kept}");
        assert_eq!(kept + rejected, in_scope);
        assert_eq!(
            count(&w, "SELECT count(*) FROM silver.usda_portion_unit"),
            kept
        );
        let read = count(
            &w,
            "SELECT count(*) FROM silver.usda_portion_unit WHERE kind IS NOT NULL",
        );
        let listed = count(
            &w,
            "SELECT coalesce(sum(portions), 0) FROM gold.portion_unread",
        );
        assert_eq!(read + listed, kept);
    }

    /// "fl oz" is a fluid ounce, a volume, not a portion in "fl" units.
    #[test]
    fn fluid_ounce_portions_are_read_as_volume() {
        let w = warehouse();
        let fl_oz = count(
            &w,
            "SELECT count(*) FROM silver.usda_portion WHERE starts_with(lower(unit_text), 'fl oz')",
        );
        assert!(fl_oz >= 400, "{fl_oz}");
        assert_eq!(
            count(
                &w,
                "SELECT count(*) FROM silver.usda_portion p JOIN silver.usda_portion_unit u USING (dataset, id) WHERE starts_with(lower(p.unit_text), 'fl oz') AND u.kind = 'volume' AND u.unit = 'fl oz'"
            ),
            fl_oz
        );
    }

    #[test]
    fn the_portions_report_is_current() {
        let w = warehouse();
        let (sources, _) = Sources::load(&repo_root()).unwrap();
        let written = fs::read_to_string(repo_root().join(PORTIONS_PATH)).unwrap_or_default();
        assert!(
            written == portions_report(&w, &sources).unwrap(),
            "{PORTIONS_PATH} is not what the warehouse writes: run `cargo run -p balaena-warehouse -- portions`"
        );
    }

    #[test]
    fn the_coverage_report_is_current() {
        let w = warehouse();
        let (sources, _) = Sources::load(&repo_root()).unwrap();
        let written = fs::read_to_string(repo_root().join(COVERAGE_PATH)).unwrap_or_default();
        assert!(
            written == coverage_report(&w, &sources).unwrap(),
            "{COVERAGE_PATH} is not what the warehouse writes: run `cargo run -p balaena-warehouse -- coverage`"
        );
    }

    #[test]
    fn the_catalogue_is_current() {
        let w = warehouse();
        let written = fs::read_to_string(repo_root().join(CATALOGUE_PATH)).unwrap_or_default();
        assert!(
            written == catalogue(&w).unwrap(),
            "{CATALOGUE_PATH} is not what the warehouse writes: run `cargo run -p balaena-warehouse -- catalogue`"
        );
    }

    #[test]
    fn the_export_is_current() {
        let w = warehouse();
        let (sources, _) = Sources::load(&repo_root()).unwrap();
        let written = fs::read_to_string(repo_root().join(export::EXPORT_PATH)).unwrap_or_default();
        assert!(
            written == export::export(&w, &sources).unwrap(),
            "{} is not what the warehouse writes: run `cargo run -p balaena-warehouse -- export`",
            export::EXPORT_PATH
        );
    }

    /// crates/core reads back each view the export carries as gold holds it: the same columns,
    /// each of the type its DuckDB type is written as, and the same rows in key order, every value
    /// equal; and the sha256s of the zips, Wikidata revisions and records the warehouse loaded.
    #[test]
    fn the_export_reads_back_as_gold() {
        use balaena_core::{Export, Type, Value};
        use duckdb::types::Value as Cell;
        let w = warehouse();
        let (sources, _) = Sources::load(&repo_root()).unwrap();
        assert_eq!(export::EXPORT_PATH, balaena_core::EXPORT_PATH);
        assert_eq!(export::PROFILE, balaena_core::PROFILE);
        let read = Export::parse(&export::export(&w, &sources).unwrap()).unwrap();
        let names: Vec<&str> = read.tables.iter().map(|t| t.name.as_str()).collect();
        let views: Vec<&str> = export::EXPORTED.iter().map(|e| e.view).collect();
        assert_eq!(names, views);
        for e in export::EXPORTED {
            let t = read.table(e.view).unwrap();
            let columns = rows(
                &w,
                &format!(
                    "SELECT column_name, data_type FROM duckdb_columns() \
                     WHERE schema_name = 'gold' AND table_name = '{}' ORDER BY column_index",
                    e.view
                ),
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .unwrap();
            let expected: Vec<(String, Type)> = columns
                .into_iter()
                .map(|(name, data_type)| {
                    let kind = match data_type.as_str() {
                        "INTEGER" | "BIGINT" => Type::Integer,
                        "DOUBLE" => Type::Number,
                        "VARCHAR" => Type::String,
                        "BOOLEAN" => Type::Boolean,
                        other => panic!("gold.{}.{name} is {other}", e.view),
                    };
                    (name, kind)
                })
                .collect();
            let fields: Vec<(String, Type)> =
                t.fields.iter().map(|f| (f.name.clone(), f.kind)).collect();
            assert_eq!(fields, expected, "gold.{}", e.view);
            let gold = rows(
                &w,
                &format!(
                    "SELECT * FROM gold.{} ORDER BY {}",
                    e.view,
                    e.key.join(", ")
                ),
                |r| {
                    (0..expected.len())
                        .map(|i| {
                            Ok(match r.get::<_, Cell>(i)? {
                                Cell::Null => Value::Null,
                                Cell::Int(n) => Value::Integer(n.into()),
                                Cell::BigInt(n) => Value::Integer(n),
                                Cell::Double(x) => Value::Number(x),
                                Cell::Text(s) => Value::String(s),
                                Cell::Boolean(b) => Value::Boolean(b),
                                other => panic!("gold.{}: {other:?}", e.view),
                            })
                        })
                        .collect::<duckdb::Result<Vec<Value>>>()
                },
            )
            .unwrap();
            assert!(!gold.is_empty(), "gold.{} has no rows", e.view);
            assert_eq!(t.rows.len(), gold.len(), "gold.{}", e.view);
            for (n, (got, want)) in t.rows.iter().zip(&gold).enumerate() {
                assert_eq!(got, want, "gold.{}, row {}", e.view, n + 1);
            }
        }
        let zips: Vec<String> = read
            .lineage
            .zips
            .iter()
            .map(|z| format!("{} {} {} {}", z.dataset, z.url, z.sha256, z.bytes))
            .collect();
        assert_eq!(
            zips,
            strings(
                &w,
                "SELECT concat_ws(' ', dataset, url, sha256, bytes) FROM bronze.usda_zip ORDER BY dataset"
            )
        );
        let revisions: Vec<String> = read
            .lineage
            .wikidata
            .iter()
            .map(|r| format!("{} {} {} {}", r.item, r.revision, r.sha256, r.bytes))
            .collect();
        assert_eq!(
            revisions,
            strings(
                &w,
                "SELECT concat_ws(' ', item, revision, sha256, bytes) FROM bronze.wikidata_revision ORDER BY item"
            )
        );
        assert!(revisions.len() >= 2, "{revisions:?}");
        let records: Vec<String> = read
            .lineage
            .records
            .iter()
            .map(|r| format!("{} {}", r.path, r.sha256))
            .collect();
        assert_eq!(
            records,
            strings(
                &w,
                "SELECT concat_ws(' ', path, sha256) FROM bronze.record_hash ORDER BY path"
            )
        );
        assert!(records.len() >= 20, "{records:?}");
    }

    #[test]
    fn the_export_is_written_only_while_every_publisher_s_license_is_cc0() {
        let w = warehouse();
        let (mut sources, _) = Sources::load(&repo_root()).unwrap();
        sources.publisher.get_mut("usda").unwrap().license = "CC-BY-4.0".into();
        let err = export::export(&w, &sources).unwrap_err();
        assert!(
            err.contains("publisher usda licenses its data as CC-BY-4.0"),
            "{err}"
        );
    }

    #[test]
    fn every_table_and_view_says_what_it_is() {
        let w = warehouse();
        let silent = strings(
            &w,
            "SELECT schema_name || '.' || table_name FROM duckdb_tables() \
             WHERE schema_name IN ('bronze', 'silver', 'gold') AND coalesce(trim(comment), '') = '' \
             UNION ALL \
             SELECT schema_name || '.' || view_name FROM duckdb_views() \
             WHERE schema_name IN ('bronze', 'silver', 'gold') AND coalesce(trim(comment), '') = ''",
        );
        assert!(silent.is_empty(), "no comment: {silent:?}");
        let described = count(
            &w,
            "SELECT count(*) FROM duckdb_tables() WHERE schema_name IN ('bronze', 'silver', 'gold')",
        );
        assert!(described >= 35, "{described}");
    }

    #[test]
    fn every_food_has_an_answer_for_each_diet() {
        let w = warehouse();
        let foods = count(&w, "SELECT count(*) FROM silver.usda_food");
        assert_eq!(count(&w, "SELECT count(*) FROM gold.food_diet"), foods);
        assert_eq!(
            count(
                &w,
                "SELECT count(*) FROM gold.food_diet WHERE vegan NOT IN ('yes', 'no', 'unknown') \
                 OR vegetarian NOT IN ('yes', 'no', 'unknown') OR vegan IS NULL OR vegetarian IS NULL"
            ),
            0
        );
    }

    /// The fixtures plan step 5 names, by USDA's names for the foods: each name must match a
    /// food, and every food it matches must take the answers given.
    #[test]
    fn the_diet_fixtures_hold() {
        let w = warehouse();
        let fixtures: [(&str, &str, &str); 22] = [
            ("Mushrooms, oyster, raw", "yes", "yes"),
            ("Mushroom, oyster", "yes", "yes"),
            ("Mushroom, king oyster", "yes", "yes"),
            ("Salsify, (vegetable oyster), raw", "yes", "yes"),
            ("Squash, summer, scallop, raw", "yes", "yes"),
            (
                "Squash, summer, scallop, cooked, boiled, drained, without salt",
                "yes",
                "yes",
            ),
            (
                "Squash, summer, scallop, cooked, boiled, drained, with salt",
                "yes",
                "yes",
            ),
            ("Egg, duck, whole, fresh, raw", "no", "yes"),
            ("Egg, turkey, whole, fresh, raw", "no", "yes"),
            ("Fish oil, cod liver", "no", "no"),
            ("Fish oil, herring", "no", "no"),
            ("Fish oil, menhaden", "no", "no"),
            ("Fish oil, menhaden, fully hydrogenated", "no", "no"),
            ("Fish oil, salmon", "no", "no"),
            ("Fish oil, sardine", "no", "no"),
            (
                "Margarine-like spread, SMART BALANCE Omega Plus Spread (with plant sterols & fish oil)",
                "no",
                "no",
            ),
            ("Cheese, parmesan, grated", "no", "unknown"),
            ("Beans, baked, canned, with franks", "no", "no"),
            ("Beverages, water, tap, drinking", "yes", "yes"),
            ("Leavening agents, baking soda", "yes", "yes"),
            (
                "Leavening agents, baking powder, double-acting, sodium aluminum sulfate",
                "yes",
                "yes",
            ),
            // Vegan unknown, decided with Jon, 2026-10-17: its name does not say cane or beet.
            // Vegetarian unknown by its category, mixed.
            ("Sugars, granulated", "unknown", "unknown"),
        ];
        for (name, vegan, vegetarian) in fixtures {
            let got = strings(
                &w,
                &format!(
                    "SELECT vegan || ' ' || vegetarian FROM gold.food_diet WHERE description = {}",
                    literal(name)
                ),
            );
            assert!(!got.is_empty(), "no food is named {name:?}");
            for answers in got {
                assert_eq!(answers, format!("{vegan} {vegetarian}"), "{name}");
            }
        }
        assert_eq!(
            count(
                &w,
                "SELECT count(DISTINCT dataset) FROM gold.food_diet WHERE description = 'Cheese, parmesan, grated'"
            ),
            2,
            "Parmesan is a fixture in both datasets"
        );
    }

    #[test]
    fn the_diet_report_is_current() {
        let w = warehouse();
        let (sources, _) = Sources::load(&repo_root()).unwrap();
        let written = fs::read_to_string(repo_root().join(DIET_PATH)).unwrap_or_default();
        assert!(
            written == diet_report(&w, &sources).unwrap(),
            "{DIET_PATH} is not what the warehouse writes: run `cargo run -p balaena-warehouse -- diet`"
        );
    }

    /// Every form names a silver food, and the load has held its name to USDA's.
    #[test]
    fn every_ingredient_form_names_a_food_in_the_pinned_release() {
        let w = warehouse();
        let forms = count(&w, "SELECT count(*) FROM silver.ingredient_form");
        assert!(forms >= 31, "{forms}");
        assert_eq!(
            count(
                &w,
                "SELECT count(*) FROM gold.ingredient_form WHERE dataset IS NOT NULL"
            ),
            forms
        );
        assert_eq!(
            count(
                &w,
                "SELECT count(*) FROM silver.ingredient i WHERE NOT EXISTS (SELECT 1 FROM silver.ingredient_form f WHERE f.ingredient = i.ingredient AND f.position = 1)"
            ),
            0,
            "every ingredient has a default form"
        );
    }

    /// The red lentil soup's ingredients, by the names its recipe uses, resolve to an ingredient
    /// and a default form.
    #[test]
    fn the_red_lentil_soup_s_ingredients_resolve() {
        let w = warehouse();
        for (alias, ingredient, state) in [
            ("red lentils", "red-lentils", "dry"),
            ("onion", "onion", "raw"),
            ("garlic", "garlic", "raw"),
            ("olive oil", "olive-oil", "as_sold"),
            ("vegetable broth", "vegetable-broth", "as_sold"),
            ("cumin", "cumin", "dry"),
            ("lemon juice", "lemon-juice", "raw"),
            ("parmesan", "parmesan", "as_sold"),
        ] {
            let got = strings(
                &w,
                &format!(
                    "SELECT a.ingredient || ' ' || f.state FROM silver.ingredient_alias a \
                     JOIN gold.ingredient_form f ON f.ingredient = a.ingredient AND f.is_default \
                     WHERE a.alias = {}",
                    literal(alias)
                ),
            );
            assert_eq!(got, [format!("{ingredient} {state}")], "{alias}");
        }
    }

    #[test]
    fn alias_candidates_come_from_usda_s_names() {
        let w = warehouse();
        assert!(
            count(
                &w,
                "SELECT count(*) FROM gold.alias_candidate WHERE ingredient = 'chickpeas' AND candidate = 'bengal gram'"
            ) >= 1
        );
        assert_eq!(
            count(
                &w,
                "SELECT count(*) FROM gold.alias_candidate WHERE starts_with(candidate, 'includes') OR candidate IN (SELECT alias FROM silver.ingredient_alias)"
            ),
            0
        );
    }

    #[test]
    fn the_crops_report_is_current() {
        let w = warehouse();
        let written = fs::read_to_string(repo_root().join(CROPS_PATH)).unwrap_or_default();
        assert!(
            written == crops_report(&w).unwrap(),
            "{CROPS_PATH} is not what the warehouse writes: run `cargo run -p balaena-warehouse -- crops`"
        );
    }

    /// An alias of two or more words whose last word is a count unit's name counts in that unit
    /// (`gold.ingredient_alias_unit`): the records' two, and, on rows inserted for the purpose and
    /// rolled back, a plural, a name of one word, a unit's name that is not last, and a whole unit.
    #[test]
    fn an_alias_ending_in_a_count_unit_s_name_counts_in_it() {
        let w = warehouse();
        let units = |w: &Connection| {
            strings(
                w,
                "SELECT alias || ' ' || unit FROM gold.ingredient_alias_unit ORDER BY alias",
            )
        };
        assert_eq!(units(&w), ["garlic clove clove", "garlic cloves clove"]);
        let inserted = w.execute_batch(
            "BEGIN; INSERT INTO silver.ingredient_alias VALUES
             ('bay leaves', 'garlic'), ('leaves', 'garlic'), ('leaves of garlic', 'garlic'),
             ('a fruit', 'lemon');",
        );
        let got = inserted.as_ref().map(|_| units(&w));
        w.execute_batch("ROLLBACK").unwrap();
        assert_eq!(
            got.unwrap(),
            [
                "a fruit fruit",
                "bay leaves leaf",
                "garlic clove clove",
                "garlic cloves clove"
            ]
        );
    }

    #[test]
    fn the_ingredients_report_is_current() {
        let w = warehouse();
        let written = fs::read_to_string(repo_root().join(INGREDIENTS_PATH)).unwrap_or_default();
        assert!(
            written == ingredients_report(&w).unwrap(),
            "{INGREDIENTS_PATH} is not what the warehouse writes: run `cargo run -p balaena-warehouse -- ingredients`"
        );
    }

    /// crates/core, which does not depend on the records, reads a recipe's note for a form's state
    /// from a list of its own: the records' states, in order.
    #[test]
    fn a_note_names_the_states_the_records_have() {
        let records: Vec<&str> = balaena_records::State::ALL
            .iter()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(balaena_core::recipe::STATES.to_vec(), records);
    }

    #[test]
    fn every_usda_row_traces_to_the_bytes_it_came_from() {
        let w = warehouse();
        let mut stmt = w
            .prepare(
                "SELECT table_name FROM information_schema.tables WHERE table_schema = 'bronze' AND table_name LIKE 'usda\\_%' ESCAPE '\\' AND table_name NOT IN ('usda_zip', 'usda_member')",
            )
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(tables.len() >= 20, "{tables:?}");
        for table in tables {
            let rows = count(&w, &format!("SELECT count(*) FROM bronze.{table}"));
            let traced = count(
                &w,
                &format!(
                    "SELECT count(*) FROM bronze.{table} t JOIN bronze.usda_member m \
                     ON m.dataset = t.dataset AND m.loaded_as = {} \
                     JOIN bronze.usda_zip z ON z.dataset = m.dataset",
                    literal(&table)
                ),
            );
            assert_eq!(
                traced, rows,
                "bronze.{table}: rows that name no member and zip"
            );
        }
    }
}
