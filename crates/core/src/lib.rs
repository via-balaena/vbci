//! What VBCI's readers share: the export, `export/datapackage.json`, read without a database. The
//! warehouse writes it (crates/warehouse, src/export.rs) as a Frictionless Data Package, version 2;
//! this crate reads it into tables and checks what the descriptor says of them: each row has its
//! fields' types, each primary key is unique, and each foreign key finds its row. `recipe` analyses
//! a recipe against it.

pub mod recipe;

use serde::Deserialize;
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// The Data Package profile an export declares.
pub const PROFILE: &str = "https://datapackage.org/profiles/2.0/datapackage.json";

/// Where the export is, relative to the repo root.
pub const EXPORT_PATH: &str = "export/datapackage.json";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Package {
    #[serde(rename = "$schema")]
    schema: String,
    name: String,
    title: String,
    description: String,
    licenses: Vec<License>,
    sources: Vec<Source>,
    #[serde(rename = "_lineage")]
    lineage: Lineage,
    resources: Vec<Resource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct License {
    pub name: String,
    pub path: String,
    pub title: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub title: String,
    pub path: String,
    pub version: Option<String>,
}

/// The sha256 of each zip and record file the export's rows came from.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lineage {
    pub zips: Vec<Zip>,
    pub records: Vec<Record>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Zip {
    pub dataset: String,
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Resource {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    description: String,
    schema: Schema,
    data: Vec<Vec<Json>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Schema {
    fields: Vec<Field>,
    primary_key: Vec<String>,
    #[serde(default)]
    foreign_keys: Vec<ForeignKey>,
}

/// A Table Schema type, of those the export writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Type {
    Integer,
    Number,
    String,
    Boolean,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: Type,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForeignKey {
    pub fields: Vec<String>,
    pub reference: Reference,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub resource: String,
    pub fields: Vec<String>,
}

/// One cell: a value of its field's type, or null.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Number(f64),
    String(String),
    Boolean(bool),
}

/// One resource of the export: its fields, keys and rows.
#[derive(Debug)]
pub struct Table {
    pub name: String,
    pub description: String,
    pub fields: Vec<Field>,
    pub primary_key: Vec<String>,
    pub foreign_keys: Vec<ForeignKey>,
    pub rows: Vec<Vec<Value>>,
}

impl Table {
    /// The position of the field named `name`.
    pub fn column(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|f| f.name == name)
    }

    /// Each row's values in the fields `names`, as one text per row; `None` for a row with a null
    /// in one of them.
    fn tuples(&self, names: &[String]) -> Result<Vec<Option<String>>, String> {
        let mut at = Vec::new();
        for n in names {
            at.push(
                self.column(n)
                    .ok_or_else(|| format!("{}: no field {n:?}", self.name))?,
            );
        }
        Ok(self
            .rows
            .iter()
            .map(|row| {
                let cells: Vec<&Value> = at.iter().map(|&i| &row[i]).collect();
                if cells.iter().any(|v| **v == Value::Null) {
                    None
                } else {
                    Some(format!("{cells:?}"))
                }
            })
            .collect())
    }
}

/// The export, read and checked.
#[derive(Debug)]
pub struct Export {
    pub name: String,
    pub title: String,
    pub description: String,
    pub licenses: Vec<License>,
    pub sources: Vec<Source>,
    pub lineage: Lineage,
    pub tables: Vec<Table>,
}

/// `json` as a value of type `kind`; `None` where it is not one.
fn value(kind: Type, json: &Json) -> Option<Value> {
    match (kind, json) {
        (_, Json::Null) => Some(Value::Null),
        (Type::Integer, Json::Number(n)) => n.as_i64().map(Value::Integer),
        (Type::Number, Json::Number(n)) => n.as_f64().map(Value::Number),
        (Type::String, Json::String(s)) => Some(Value::String(s.clone())),
        (Type::Boolean, Json::Bool(b)) => Some(Value::Boolean(*b)),
        _ => None,
    }
}

impl Export {
    /// Reads `export/datapackage.json` under `root`.
    pub fn load(root: &Path) -> Result<Export, String> {
        let text = fs::read_to_string(root.join(EXPORT_PATH))
            .map_err(|e| format!("{EXPORT_PATH}: {e}"))?;
        Self::parse(&text).map_err(|e| format!("{EXPORT_PATH}: {e}"))
    }

    /// Parses an export's text and checks its tables: every resource is a table, listed once, whose
    /// fields' names are each listed once and make its first row; every other row has a value of
    /// its field's type, or null, in each field; a primary key's values are never null and never
    /// repeated; a foreign key refers to its table's primary key, and its values are all present
    /// and found there.
    pub fn parse(text: &str) -> Result<Export, String> {
        let p: Package = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if p.schema != PROFILE {
            return Err(format!(
                "$schema is {:?}: an export is a Data Package declaring {PROFILE}",
                p.schema
            ));
        }
        let mut tables = Vec::new();
        let mut names = BTreeSet::new();
        for r in p.resources {
            let at = |e: String| format!("resource {}: {e}", r.name);
            if !names.insert(r.name.clone()) {
                return Err(at("listed twice".into()));
            }
            if r.kind != "table" {
                return Err(at(format!("type is {:?}, not \"table\"", r.kind)));
            }
            let fields = r.schema.fields;
            let mut field_names = BTreeSet::new();
            for f in &fields {
                if !field_names.insert(f.name.as_str()) {
                    return Err(at(format!("field {:?} is listed twice", f.name)));
                }
            }
            let mut data = r.data.into_iter();
            let header: Vec<Json> = data.next().unwrap_or_default();
            let names: Vec<Json> = fields.iter().map(|f| f.name.clone().into()).collect();
            if header != names {
                return Err(at("its first row is not its fields' names, in order".into()));
            }
            let mut rows = Vec::new();
            for (n, row) in data.enumerate() {
                if row.len() != fields.len() {
                    return Err(at(format!(
                        "row {} has {} values for {} fields",
                        n + 1,
                        row.len(),
                        fields.len()
                    )));
                }
                let mut cells = Vec::with_capacity(row.len());
                for (f, json) in fields.iter().zip(&row) {
                    cells.push(value(f.kind, json).ok_or_else(|| {
                        at(format!(
                            "row {}, {}: {json} is not {:?}",
                            n + 1,
                            f.name,
                            f.kind
                        ))
                    })?);
                }
                rows.push(cells);
            }
            tables.push(Table {
                name: r.name,
                description: r.description,
                fields,
                primary_key: r.schema.primary_key,
                foreign_keys: r.schema.foreign_keys,
                rows,
            });
        }
        let mut keys: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
        for t in &tables {
            if t.primary_key.is_empty() {
                return Err(format!("resource {}: no primary key", t.name));
            }
            let mut seen = BTreeSet::new();
            for (n, tuple) in t.tuples(&t.primary_key)?.into_iter().enumerate() {
                let tuple = tuple.ok_or_else(|| {
                    format!(
                        "resource {}: row {} has a null in its primary key",
                        t.name,
                        n + 1
                    )
                })?;
                if !seen.insert(tuple.clone()) {
                    return Err(format!(
                        "resource {}: row {} repeats the primary key {tuple}",
                        t.name,
                        n + 1
                    ));
                }
            }
            keys.insert(&t.name, seen);
        }
        for t in &tables {
            for fk in &t.foreign_keys {
                let target = tables
                    .iter()
                    .find(|o| o.name == fk.reference.resource)
                    .ok_or_else(|| {
                        format!(
                            "resource {}: a foreign key names resource {:?}, which the export lacks",
                            t.name, fk.reference.resource
                        )
                    })?;
                if fk.reference.fields != target.primary_key {
                    return Err(format!(
                        "resource {}: a foreign key refers to {:?} in {}, which is not its primary key",
                        t.name, fk.reference.fields, target.name
                    ));
                }
                let found = &keys[target.name.as_str()];
                for (n, tuple) in t.tuples(&fk.fields)?.into_iter().enumerate() {
                    if !tuple.as_ref().is_some_and(|k| found.contains(k)) {
                        return Err(format!(
                            "resource {}: row {}'s {:?} finds no row in {}",
                            t.name,
                            n + 1,
                            fk.fields,
                            target.name
                        ));
                    }
                }
            }
        }
        Ok(Export {
            name: p.name,
            title: p.title,
            description: p.description,
            licenses: p.licenses,
            sources: p.sources,
            lineage: p.lineage,
            tables,
        })
    }

    /// The table named `name`.
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|t| t.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two tables, one referring to the other, in the export's form.
    const SMALL: &str = r#"{
  "$schema": "https://datapackage.org/profiles/2.0/datapackage.json",
  "name": "small",
  "title": "Small",
  "description": "Two tables.",
  "licenses": [{"name": "CC0-1.0", "path": "https://creativecommons.org/publicdomain/zero/1.0/", "title": "CC0 1.0 Universal"}],
  "sources": [{"title": "A test", "path": "https://example.org"}],
  "_lineage": {"zips": [], "records": []},
  "resources": [
    {
      "name": "food",
      "type": "table",
      "description": "Foods.",
      "schema": {
        "fields": [
          {"name": "fdc_id", "type": "integer"},
          {"name": "description", "type": "string"},
          {"name": "vegan", "type": "boolean"}
        ],
        "primaryKey": ["fdc_id"]
      },
      "data": [
        ["fdc_id","description","vegan"],
        [1,"Garlic, raw",true],
        [2,"Cheese, parmesan, grated",null]
      ]
    },
    {
      "name": "portion",
      "type": "table",
      "description": "Portions.",
      "schema": {
        "fields": [
          {"name": "id", "type": "string"},
          {"name": "fdc_id", "type": "integer"},
          {"name": "grams", "type": "number", "description": "Grams in one."}
        ],
        "primaryKey": ["id"],
        "foreignKeys": [{"fields": ["fdc_id"], "reference": {"resource": "food", "fields": ["fdc_id"]}}]
      },
      "data": [
        ["id","fdc_id","grams"],
        ["a",1,3.0],
        ["b",2,0.1]
      ]
    }
  ]
}"#;

    fn with(old: &str, new: &str) -> String {
        assert_eq!(SMALL.matches(old).count(), 1, "{old:?}");
        SMALL.replace(old, new)
    }

    fn refused(text: &str, why: &str) {
        let err = Export::parse(text).expect_err("refused");
        assert!(err.contains(why), "{err:?} does not say {why:?}");
    }

    #[test]
    fn a_good_export_reads_into_typed_tables() {
        let e = Export::parse(SMALL).unwrap();
        let food = e.table("food").unwrap();
        assert_eq!(food.primary_key, ["fdc_id"]);
        assert_eq!(
            food.rows[0],
            [
                Value::Integer(1),
                Value::String("Garlic, raw".into()),
                Value::Boolean(true)
            ]
        );
        assert_eq!(food.rows[1][2], Value::Null);
        let portion = e.table("portion").unwrap();
        assert_eq!(portion.rows[1][2], Value::Number(0.1));
        assert_eq!(portion.column("grams"), Some(2));
        assert_eq!(portion.foreign_keys[0].reference.resource, "food");
    }

    #[test]
    fn an_export_is_a_v2_data_package_with_keys_it_knows() {
        refused(
            &with(
                "profiles/2.0/datapackage.json",
                "profiles/1.0/datapackage.json",
            ),
            "$schema is",
        );
        refused(
            &with(
                "\"name\": \"small\",",
                "\"name\": \"small\",\n  \"version\": \"1.0.0\",",
            ),
            "unknown field `version`",
        );
        refused(
            &with(
                "\"description\": \"Foods.\",",
                "\"description\": \"Foods.\", \"path\": \"food.csv\",",
            ),
            "unknown field `path`",
        );
        refused(
            &with(
                "{\"name\": \"vegan\", \"type\": \"boolean\"}",
                "{\"name\": \"vegan\", \"type\": \"date\"}",
            ),
            "unknown variant `date`",
        );
        refused(
            &with(
                "\"name\": \"portion\",\n      \"type\": \"table\"",
                "\"name\": \"portion\",\n      \"type\": \"file\"",
            ),
            "type is \"file\", not \"table\"",
        );
        refused(
            &with(
                "\"description\": \"Grams in one.\"}",
                "\"description\": \"Grams in one.\", \"format\": \"default\"}",
            ),
            "unknown field `format`",
        );
        refused(
            &with(
                "\"primaryKey\": [\"id\"],",
                "\"primaryKey\": [\"id\"], \"missingValues\": [\"NA\"],",
            ),
            "unknown field `missingValues`",
        );
        refused(
            &with(
                "\"title\": \"Small\",",
                "\"title\": \"Small\",\n  \"title\": \"Other\",",
            ),
            "duplicate field `title`",
        );
        refused(
            &with("\"records\": []}", "\"records\": [], \"commit\": \"x\"}"),
            "unknown field `commit`",
        );
        // Without its foreign key, so that the second "food" breaks nothing else.
        let no_fk = with(
            "\"primaryKey\": [\"id\"],\n        \"foreignKeys\": [{\"fields\": [\"fdc_id\"], \"reference\": {\"resource\": \"food\", \"fields\": [\"fdc_id\"]}}]",
            "\"primaryKey\": [\"id\"]",
        );
        assert_eq!(no_fk.matches("\"name\": \"portion\"").count(), 1);
        refused(
            &no_fk.replace("\"name\": \"portion\"", "\"name\": \"food\""),
            "resource food: listed twice",
        );
    }

    #[test]
    fn each_row_has_a_value_of_its_field_s_type_in_each_field() {
        // A second field named description, its header and rows to match.
        let twice = with(
            "{\"name\": \"vegan\", \"type\": \"boolean\"}",
            "{\"name\": \"description\", \"type\": \"string\"}",
        )
        .replace(
            "[\"fdc_id\",\"description\",\"vegan\"]",
            "[\"fdc_id\",\"description\",\"description\"]",
        )
        .replace("\",true]", "\",\"x\"]")
        .replace("\",null]", "\",\"y\"]");
        refused(
            &twice,
            "resource food: field \"description\" is listed twice",
        );
        refused(
            &with(
                "[\"fdc_id\",\"description\",\"vegan\"]",
                "[\"fdc_id\",\"vegan\",\"description\"]",
            ),
            "its first row is not its fields' names",
        );
        refused(
            &with("[1,\"Garlic, raw\",true]", "[1,\"Garlic, raw\"]"),
            "row 1 has 2 values for 3 fields",
        );
        for (old, new, why) in [
            (
                "[1,\"Garlic, raw\",true]",
                "[\"1\",\"Garlic, raw\",true]",
                "row 1, fdc_id: \"1\" is not Integer",
            ),
            (
                "[1,\"Garlic, raw\",true]",
                "[1.0,\"Garlic, raw\",true]",
                "row 1, fdc_id: 1.0 is not Integer",
            ),
            (
                "[1,\"Garlic, raw\",true]",
                "[1,7,true]",
                "row 1, description: 7 is not String",
            ),
            (
                "[1,\"Garlic, raw\",true]",
                "[1,\"Garlic, raw\",\"yes\"]",
                "row 1, vegan: \"yes\" is not Boolean",
            ),
            (
                "[\"a\",1,3.0]",
                "[\"a\",1,\"3.0\"]",
                "row 1, grams: \"3.0\" is not Number",
            ),
        ] {
            refused(&with(old, new), why);
        }
    }

    #[test]
    fn a_primary_key_is_unique_and_never_null() {
        // A third row repeats the first's key; every foreign key still finds its row.
        refused(
            &with(
                "[2,\"Cheese, parmesan, grated\",null]",
                "[2,\"Cheese, parmesan, grated\",null],\n        [1,\"Garlic\",false]",
            ),
            "resource food: row 3 repeats the primary key",
        );
        refused(
            &with("[\"b\",2,0.1]", "[null,2,0.1]"),
            "row 2 has a null in its primary key",
        );
        refused(
            &with("\"primaryKey\": [\"id\"]", "\"primaryKey\": []"),
            "resource portion: no primary key",
        );
        refused(
            &with("\"primaryKey\": [\"id\"]", "\"primaryKey\": [\"code\"]"),
            "portion: no field \"code\"",
        );
    }

    #[test]
    fn a_foreign_key_finds_its_row() {
        refused(
            &with("[\"b\",2,0.1]", "[\"b\",3,0.1]"),
            "row 2's [\"fdc_id\"] finds no row in food",
        );
        refused(
            &with("[\"b\",2,0.1]", "[\"b\",null,0.1]"),
            "row 2's [\"fdc_id\"] finds no row in food",
        );
        refused(
            &with("\"resource\": \"food\"", "\"resource\": \"foods\""),
            "names resource \"foods\", which the export lacks",
        );
        refused(
            &with(
                "\"reference\": {\"resource\": \"food\", \"fields\": [\"fdc_id\"]}",
                "\"reference\": {\"resource\": \"food\", \"fields\": [\"description\"]}",
            ),
            "which is not its primary key",
        );
    }

    #[test]
    fn the_export_loads() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let e = Export::load(&root).unwrap();
        let names: Vec<&str> = e.tables.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "food_diet",
                "nutrient",
                "food_nutrient",
                "portion",
                "count_unit_name",
                "ingredient",
                "ingredient_alias",
                "ingredient_form"
            ]
        );
        assert!(e.table("food_diet").unwrap().rows.len() >= 8000);
        assert!(e.table("food_nutrient").unwrap().rows.len() >= 100_000);
        assert_eq!(e.lineage.zips.len(), 2);
        let alias = e.table("ingredient_alias").unwrap();
        assert!(alias.rows.contains(&vec![
            Value::String("red lentils".into()),
            Value::String("red-lentils".into())
        ]));
    }
}
