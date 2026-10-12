//! `export/datapackage.json`: the gold views readers need, written to one file that crates/core
//! reads without a database. The file is a Frictionless Data Package, version 2
//! (https://datapackage.org/standard/data-package/, read 2026-10-12): each view is a resource whose
//! rows are inline, as arrays, its columns' names first (Table Dialect's `header`, true by
//! default), then one row per line in the order of its primary key, so a diff of the file reads
//! row by row. `_lineage`, a private property (the spec's `_` prefix for what software writes),
//! gives the sha256 of each zip and record file the rows came from.

use crate::{literal, rows};
use balaena_records::Sources;
use duckdb::Connection;
use duckdb::types::Value as Cell;
use serde::Serialize;
use serde_json::Value;

/// Where `export` writes the file, relative to the repo root.
pub const EXPORT_PATH: &str = "export/datapackage.json";

/// The Data Package profile the file declares.
pub const PROFILE: &str = "https://datapackage.org/profiles/2.0/datapackage.json";

/// The export's license, which it may carry only while every publisher's is the same.
const LICENSE: &str = "CC0-1.0";

/// A reference from some of a view's columns to another exported view's.
pub struct Reference {
    pub columns: &'static [&'static str],
    pub view: &'static str,
    pub to: &'static [&'static str],
}

/// A gold view the export carries, every column of it, with the columns that key it.
pub struct Exported {
    pub view: &'static str,
    pub key: &'static [&'static str],
    pub references: &'static [Reference],
}

const FOOD: Reference = Reference {
    columns: &["fdc_id"],
    view: "food_diet",
    to: &["fdc_id"],
};

const INGREDIENT: Reference = Reference {
    columns: &["ingredient"],
    view: "ingredient",
    to: &["ingredient"],
};

/// The views the export carries, in its order.
pub const EXPORTED: &[Exported] = &[
    Exported {
        view: "food_diet",
        key: &["fdc_id"],
        references: &[],
    },
    Exported {
        view: "nutrient",
        key: &["nutrient"],
        references: &[],
    },
    Exported {
        view: "food_nutrient",
        key: &["fdc_id", "nutrient"],
        references: &[
            FOOD,
            Reference {
                columns: &["nutrient"],
                view: "nutrient",
                to: &["nutrient"],
            },
        ],
    },
    Exported {
        view: "portion",
        key: &["dataset", "id"],
        references: &[FOOD],
    },
    Exported {
        view: "count_unit_name",
        key: &["name"],
        references: &[],
    },
    Exported {
        view: "ingredient",
        key: &["ingredient"],
        references: &[],
    },
    Exported {
        view: "ingredient_alias",
        key: &["alias"],
        references: &[INGREDIENT],
    },
    Exported {
        view: "ingredient_form",
        key: &["ingredient", "state"],
        references: &[INGREDIENT, FOOD],
    },
];

#[derive(Serialize)]
struct Package {
    #[serde(rename = "$schema")]
    schema: &'static str,
    name: &'static str,
    title: &'static str,
    description: String,
    licenses: Vec<License>,
    sources: Vec<Source>,
    #[serde(rename = "_lineage")]
    lineage: Lineage,
    resources: Vec<Resource>,
}

#[derive(Serialize)]
struct License {
    name: &'static str,
    path: &'static str,
    title: &'static str,
}

#[derive(Serialize)]
struct Source {
    title: String,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

#[derive(Serialize)]
struct Lineage {
    zips: Vec<Zip>,
    records: Vec<Record>,
}

#[derive(Serialize)]
struct Zip {
    dataset: String,
    url: String,
    sha256: String,
    bytes: i64,
}

#[derive(Serialize)]
struct Record {
    path: String,
    sha256: String,
}

#[derive(Serialize)]
struct Resource {
    name: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    description: String,
    schema: Schema,
    /// A placeholder, which `export` replaces with the rows, one per line.
    data: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Schema {
    fields: Vec<Field>,
    primary_key: &'static [&'static str],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    foreign_keys: Vec<ForeignKey>,
}

#[derive(Serialize)]
struct Field {
    name: String,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Serialize)]
struct ForeignKey {
    fields: &'static [&'static str],
    reference: ForeignReference,
}

#[derive(Serialize)]
struct ForeignReference {
    resource: &'static str,
    fields: &'static [&'static str],
}

/// The Table Schema type a DuckDB column is written as; `None` for a type the export does not
/// write.
fn field_type(data_type: &str) -> Option<&'static str> {
    match data_type {
        "INTEGER" | "BIGINT" => Some("integer"),
        "DOUBLE" => Some("number"),
        "VARCHAR" => Some("string"),
        "BOOLEAN" => Some("boolean"),
        _ => None,
    }
}

/// One cell, as JSON: a native JSON value of its field's type, or null.
fn cell(column: &str, kind: &str, c: Cell) -> Result<Value, String> {
    match (kind, c) {
        (_, Cell::Null) => Ok(Value::Null),
        ("integer", Cell::Int(n)) => Ok(n.into()),
        ("integer", Cell::BigInt(n)) => Ok(n.into()),
        ("number", Cell::Double(x)) => serde_json::Number::from_f64(x)
            .map(Value::Number)
            .ok_or_else(|| format!("{column}: {x} has no JSON number")),
        ("string", Cell::Text(s)) => Ok(s.into()),
        ("boolean", Cell::Boolean(b)) => Ok(b.into()),
        (_, other) => Err(format!("{column}: {other:?} is not {kind}")),
    }
}

/// One view as a resource, with its rows as JSON arrays, its columns' names first.
fn resource(conn: &Connection, e: &Exported) -> Result<(Resource, Vec<String>), String> {
    let comments = rows(
        conn,
        &format!(
            "SELECT coalesce(comment, '') FROM duckdb_views() \
             WHERE schema_name = 'gold' AND view_name = {}",
            literal(e.view)
        ),
        |r| r.get::<_, String>(0),
    )?;
    let [description] = comments.as_slice() else {
        return Err(format!("gold.{} is not a view", e.view));
    };
    let columns = rows(
        conn,
        &format!(
            "SELECT column_name, data_type, comment FROM duckdb_columns() \
             WHERE schema_name = 'gold' AND table_name = {} ORDER BY column_index",
            literal(e.view)
        ),
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        },
    )?;
    let mut fields = Vec::new();
    for (name, data_type, comment) in columns {
        let kind = field_type(&data_type).ok_or_else(|| {
            format!(
                "gold.{}.{name} is {data_type}, a type the export does not write",
                e.view
            )
        })?;
        fields.push(Field {
            name,
            kind,
            description: comment.filter(|c| !c.trim().is_empty()),
        });
    }
    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    let mut lines = vec![serde_json::to_string(&names).map_err(|e| e.to_string())?];
    let query = format!(
        "SELECT * FROM gold.{} ORDER BY {}",
        e.view,
        e.key.join(", ")
    );
    let mut stmt = conn.prepare(&query).map_err(|e| format!("{query}: {e}"))?;
    let mut result = stmt.query([]).map_err(|e| format!("{query}: {e}"))?;
    while let Some(row) = result.next().map_err(|e| format!("{query}: {e}"))? {
        let mut cells = Vec::with_capacity(fields.len());
        for (i, f) in fields.iter().enumerate() {
            let c = row.get::<_, Cell>(i).map_err(|e| format!("{query}: {e}"))?;
            cells.push(cell(&format!("gold.{}.{}", e.view, f.name), f.kind, c)?);
        }
        lines.push(serde_json::to_string(&cells).map_err(|e| e.to_string())?);
    }
    let foreign_keys = e
        .references
        .iter()
        .map(|r| ForeignKey {
            fields: r.columns,
            reference: ForeignReference {
                resource: r.view,
                fields: r.to,
            },
        })
        .collect();
    let resource = Resource {
        name: e.view,
        kind: "table",
        description: description.clone(),
        schema: Schema {
            fields,
            primary_key: e.key,
            foreign_keys,
        },
        data: placeholder(e.view),
    };
    Ok((resource, lines))
}

fn placeholder(view: &str) -> String {
    format!("<the rows of {view}>")
}

/// `export/datapackage.json`, as the warehouse writes it: every view `EXPORTED` names, with the
/// sha256s of the zips and records loaded. Refused while some publisher's license is not CC0's,
/// as the file is licensed as CC0 whole.
pub fn export(conn: &Connection, sources: &Sources) -> Result<String, String> {
    for (key, p) in &sources.publisher {
        if p.license != LICENSE {
            return Err(format!(
                "publisher {key} licenses its data as {}: the export is {LICENSE} whole, so it is not written",
                p.license
            ));
        }
    }
    let zips = rows(
        conn,
        "SELECT dataset, url, sha256, bytes FROM bronze.usda_zip ORDER BY dataset",
        |r| {
            Ok(Zip {
                dataset: r.get(0)?,
                url: r.get(1)?,
                sha256: r.get(2)?,
                bytes: r.get(3)?,
            })
        },
    )?;
    let records = rows(
        conn,
        "SELECT path, sha256 FROM bronze.record_hash ORDER BY path",
        |r| {
            Ok(Record {
                path: r.get(0)?,
                sha256: r.get(1)?,
            })
        },
    )?;
    let mut sources_out = Vec::new();
    for d in &sources.dataset {
        let publisher = &sources.publisher[&d.publisher];
        sources_out.push(Source {
            title: format!("{}, {}", d.name, publisher.name),
            path: d.url.clone(),
            version: Some(d.release.clone()),
        });
    }
    sources_out.push(Source {
        title: "VBCI's records, at the sha256s _lineage gives".into(),
        path: "https://github.com/via-balaena/vbci".into(),
        version: None,
    });
    let mut description = String::from(
        "VBCI's reference data: the views of its warehouse's gold layer that readers need, one \
         resource each. Generated by `cargo run -p balaena-warehouse -- export`; the test \
         `the_export_is_current` fails when this file differs from what that would write. \
         Change the records, not this file.\n\n\
         Each resource's rows are inline, its fields' names first. `_lineage` gives the sha256 \
         of each zip and record file the rows came from.\n\n\
         Each publisher's suggested citation, as `sources.toml` gives it:\n",
    );
    for p in sources.publisher.values() {
        description.push_str(&format!("\n- {}", p.citation));
    }
    let mut resources = Vec::new();
    let mut data = Vec::new();
    for e in EXPORTED {
        let (resource, lines) = resource(conn, e)?;
        resources.push(resource);
        data.push(lines);
    }
    let package = Package {
        schema: PROFILE,
        name: "vbci",
        title: "Via Balaena Culinary Institute: reference data",
        description,
        licenses: vec![License {
            name: LICENSE,
            path: "https://creativecommons.org/publicdomain/zero/1.0/",
            title: "CC0 1.0 Universal",
        }],
        sources: sources_out,
        lineage: Lineage { zips, records },
        resources,
    };
    let mut text = serde_json::to_string_pretty(&package).map_err(|e| e.to_string())?;
    for (e, lines) in EXPORTED.iter().zip(data) {
        let mark = serde_json::to_string(&placeholder(e.view)).map_err(|e| e.to_string())?;
        let found: Vec<usize> = text.match_indices(&mark).map(|(at, _)| at).collect();
        let [at] = found[..] else {
            return Err(format!(
                "{mark} is in the descriptor {} times, not once",
                found.len()
            ));
        };
        let line = &text[text[..at].rfind('\n').map_or(0, |n| n + 1)..at];
        let indent = " ".repeat(line.len() - line.trim_start().len());
        let rows = format!(
            "[\n{indent}  {}\n{indent}]",
            lines.join(&format!(",\n{indent}  "))
        );
        text.replace_range(at..at + mark.len(), &rows);
    }
    text.push('\n');
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gold(views: &str) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!("CREATE SCHEMA gold; {views}"))
            .unwrap();
        conn
    }

    const T: Exported = Exported {
        view: "t",
        key: &["k"],
        references: &[],
    };

    #[test]
    fn a_view_is_written_as_its_columns_names_then_a_row_per_line_in_key_order() {
        let conn = gold(
            "CREATE VIEW gold.t AS SELECT * FROM (VALUES \
             (2, 'b', CAST(0.1 AS DOUBLE), true, CAST(7 AS BIGINT)), \
             (1, NULL, CAST(3 AS DOUBLE), NULL, NULL)) v(k, s, x, b, n); \
             COMMENT ON VIEW gold.t IS 'A test.'; \
             COMMENT ON COLUMN gold.t.x IS 'Grams.';",
        );
        let (r, lines) = resource(&conn, &T).unwrap();
        assert_eq!(
            lines,
            [
                r#"["k","s","x","b","n"]"#,
                "[1,null,3.0,null,null]",
                r#"[2,"b",0.1,true,7]"#
            ]
        );
        assert_eq!(r.description, "A test.");
        let fields: Vec<(&str, &str, Option<&str>)> = r
            .schema
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.kind, f.description.as_deref()))
            .collect();
        assert_eq!(
            fields,
            [
                ("k", "integer", None),
                ("s", "string", None),
                ("x", "number", Some("Grams.")),
                ("b", "boolean", None),
                ("n", "integer", None)
            ]
        );
    }

    #[test]
    fn a_column_of_a_type_the_export_does_not_write_is_refused() {
        for (column, data_type) in [
            ("DATE '2026-10-12'", "DATE"),
            ("1.5", "DECIMAL(2,1)"),
            ("CAST(1 AS SMALLINT)", "SMALLINT"),
        ] {
            let conn = gold(&format!(
                "CREATE VIEW gold.t AS SELECT 1 AS k, {column} AS c; COMMENT ON VIEW gold.t IS 'A test.';"
            ));
            let err = resource(&conn, &T).map(|_| ()).unwrap_err();
            assert!(
                err.contains(&format!(
                    "gold.t.c is {data_type}, a type the export does not write"
                )),
                "{err}"
            );
        }
    }
}
