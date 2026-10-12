//! `balaena`, VBCI's command line. It reads only the export, `export/datapackage.json`, built into
//! the binary when it is compiled, so it carries the data it was built with.
//!
//! - `analyze <recipe.cook>`: the recipe's report, as `docs/recipes/` holds VBCI's own.
//! - `search <words>... [-n N]`: the ingredients and USDA foods whose names match.
//! - `info`: what the export came from, and how many rows each of its tables has.

use balaena_core::Export;
use balaena_core::recipe::analyze;
use balaena_core::search::search;
use clap::{Arg, ArgMatches, Command, value_parser};
use std::process::ExitCode;

/// The export the binary was built with.
const EXPORT: &str = include_str!("../../../export/datapackage.json");

fn cli() -> Command {
    Command::new("balaena")
        .about("VBCI's recipes and foods, read from the export built into this binary")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("analyze")
                .about("A Cooklang recipe's foods, grams, nutrients and diet answers")
                .arg(
                    Arg::new("recipe")
                        .required(true)
                        .help("The recipe's .cook file"),
                ),
        )
        .subcommand(
            Command::new("search")
                .about("Ingredients and USDA foods whose names have words starting with each word")
                .arg(Arg::new("words").required(true).num_args(1..))
                .arg(
                    Arg::new("n")
                        .short('n')
                        .value_parser(value_parser!(usize))
                        .default_value("20")
                        .help("How many foods to show, the simplest names first"),
                ),
        )
        .subcommand(Command::new("info").about("What the export came from, and its tables' rows"))
}

/// A Markdown table cell's text.
fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

fn found(export: &Export, words: &[String], shown: usize) -> Result<String, String> {
    let query = words.join(" ");
    let f = search(export, &query)?;
    let mut out = format!(
        "# Search: {query}\n\n## Ingredients: {}\n\n",
        f.ingredients.len()
    );
    if f.ingredients.is_empty() {
        out.push_str("None.\n");
    } else {
        out.push_str("| ingredient | aliases that match |\n|---|---|\n");
        for (id, aliases) in &f.ingredients {
            out.push_str(&format!("| {id} | {} |\n", cell(&aliases.join(", "))));
        }
    }
    let total = f.foods.len();
    if total > shown {
        out.push_str(&format!(
            "\n## USDA foods: {total}, the {shown} simplest shown\n\n"
        ));
    } else {
        out.push_str(&format!("\n## USDA foods: {total}, simplest first\n\n"));
    }
    if total == 0 {
        out.push_str("None.\n");
        return Ok(out);
    }
    out.push_str("| fdc_id | dataset | name | vegan | vegetarian | linked as |\n");
    out.push_str("|---|---|---|---|---|---|\n");
    for food in f.foods.iter().take(shown) {
        let linked: Vec<String> = food
            .forms
            .iter()
            .map(|(i, s)| format!("{i}, {}", s.replace('_', " ")))
            .collect();
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            food.fdc_id,
            food.dataset,
            cell(&food.description),
            food.vegan,
            food.vegetarian,
            cell(&linked.join("; "))
        ));
    }
    Ok(out)
}

fn info(export: &Export) -> String {
    let mut out = format!(
        "# {}\n\nFrom `export/datapackage.json`, built into this binary. It describes itself:\n\n",
        export.title
    );
    for line in export.description.lines() {
        out.push_str(&format!("> {line}\n").replace("> \n", ">\n"));
    }
    out.push_str("\n## License\n\n");
    for l in &export.licenses {
        out.push_str(&format!("- {} ({}): {}\n", l.title, l.name, l.path));
    }
    out.push_str("\n## Sources\n\n");
    for s in &export.sources {
        match &s.version {
            Some(v) => out.push_str(&format!("- {}, {v}: {}\n", s.title, s.path)),
            None => out.push_str(&format!("- {}: {}\n", s.title, s.path)),
        }
    }
    out.push_str("\n## USDA's zips\n\n| dataset | sha256 | bytes |\n|---|---|---|\n");
    for z in &export.lineage.zips {
        out.push_str(&format!("| {} | {} | {} |\n", z.dataset, z.sha256, z.bytes));
    }
    out.push_str(&format!(
        "\nRecord files: {}, each with its sha256 in the export's `_lineage`.\n\n\
         ## Tables\n\n| table | rows |\n|---|---|\n",
        export.lineage.records.len()
    ));
    for t in &export.tables {
        out.push_str(&format!("| {} | {} |\n", t.name, t.rows.len()));
    }
    out
}

fn run(m: &ArgMatches) -> Result<String, String> {
    let export = Export::parse(EXPORT).map_err(|e| format!("the export built in: {e}"))?;
    match m.subcommand() {
        Some(("analyze", a)) => {
            let path = a.get_one::<String>("recipe").expect("required");
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            let report = analyze(&text, &export).map_err(|e| format!("{path}: {e}"))?;
            Ok(report.markdown(path))
        }
        Some(("search", a)) => {
            let words: Vec<String> = a
                .get_many::<String>("words")
                .expect("required")
                .cloned()
                .collect();
            found(
                &export,
                &words,
                *a.get_one::<usize>("n").expect("defaulted"),
            )
        }
        Some(("info", _)) => Ok(info(&export)),
        _ => unreachable!("clap requires a subcommand"),
    }
}

fn main() -> ExitCode {
    match run(&cli().get_matches()) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
