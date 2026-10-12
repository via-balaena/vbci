//! Checks on the repo itself: the rules in docs/architecture.md that span crates. There is no
//! library here, only tests.

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use toml::{Table, Value};

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn manifest(path: &Path) -> Table {
        fs::read_to_string(path).unwrap().parse().unwrap()
    }

    /// Every crate in the workspace, as its manifest's `members` names them: a directory, or
    /// `<dir>/*` for each directory under it that has a manifest. Relative to the root.
    fn members() -> Vec<String> {
        let ws = manifest(&root().join("Cargo.toml"));
        let mut out = Vec::new();
        for m in ws["workspace"]["members"].as_array().unwrap() {
            let m = m.as_str().unwrap();
            let Some(parent) = m.strip_suffix("/*") else {
                out.push(m.to_string());
                continue;
            };
            for e in fs::read_dir(root().join(parent)).unwrap() {
                let name = e.unwrap().file_name().to_string_lossy().into_owned();
                if root().join(parent).join(&name).join("Cargo.toml").is_file() {
                    out.push(format!("{parent}/{name}"));
                }
            }
        }
        out.sort();
        out
    }

    /// The workspace's manifest and every crate's, by path.
    fn manifests() -> Vec<(String, Table)> {
        let mut out = vec![(
            "Cargo.toml".to_string(),
            manifest(&root().join("Cargo.toml")),
        )];
        for m in members() {
            let path = format!("{m}/Cargo.toml");
            out.push((path.clone(), manifest(&root().join(path))));
        }
        out
    }

    /// Every dependency a manifest declares, of every kind, with its spec.
    fn dependencies(m: &Table) -> Vec<(String, Value)> {
        let mut tables: Vec<&Table> = Vec::new();
        for kind in ["dependencies", "dev-dependencies", "build-dependencies"] {
            tables.extend(m.get(kind).and_then(Value::as_table));
            if let Some(targets) = m.get("target").and_then(Value::as_table) {
                for t in targets.values().filter_map(Value::as_table) {
                    tables.extend(t.get(kind).and_then(Value::as_table));
                }
            }
        }
        if let Some(ws) = m.get("workspace").and_then(Value::as_table) {
            tables.extend(ws.get("dependencies").and_then(Value::as_table));
        }
        tables
            .into_iter()
            .flat_map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())))
            .collect()
    }

    /// docs/architecture.md names the tests that hold its rules, and the files it describes. Up to
    /// its plan, which is about what does not exist yet, each must exist: a renamed test or a moved
    /// file fails here, not silently in the doc.
    #[test]
    fn the_architecture_doc_names_real_tests_and_files() {
        let doc = fs::read_to_string(root().join("docs/architecture.md")).unwrap();
        let (now, _) = doc
            .split_once("\n## Plan")
            .expect("docs/architecture.md has a Plan section");
        // Fenced blocks are not prose, and their ``` would pair with the wrong `.
        let mut fenced = false;
        let now: String = now
            .lines()
            .filter(|line| {
                if line.starts_with("```") {
                    fenced = !fenced;
                    return false;
                }
                !fenced
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut source = String::new();
        let mut stack: Vec<PathBuf> = members().iter().map(|m| root().join(m)).collect();
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    source.push_str(&fs::read_to_string(&path).unwrap());
                }
            }
        }
        let (mut tests, mut paths) = (0, 0);
        for (i, token) in now.split('`').enumerate() {
            if i % 2 == 0 {
                continue;
            }
            let is_name = token.matches('_').count() >= 2
                && token
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            if is_name {
                tests += 1;
                assert!(
                    source.contains(&format!("fn {token}()")),
                    "docs/architecture.md names the test {token}, which does not exist"
                );
            } else if token.contains('<') || token.starts_with('$') || token.starts_with('~') {
                // A pattern such as `recipes/<name>.cook`, or a path under an environment
                // variable or a home directory such as `$HOME/.cache/vbci`: not a file in the repo.
            } else if token.contains('/') || token.ends_with(".toml") || token.ends_with(".md") {
                paths += 1;
                let path = root().join(token);
                let in_docs = root().join("docs").join(token);
                assert!(
                    path.exists() || in_docs.exists(),
                    "docs/architecture.md names {token}, which does not exist"
                );
            }
        }
        assert!(
            tests >= 7 && paths >= 4,
            "found {tests} tests and {paths} paths"
        );
    }

    /// The paths a markdown text names in backticks, each as the file or the folder it lies in: a
    /// pattern such as `recipes/<name>.cook` is the folder `recipes/`.
    fn record_paths(text: &str) -> BTreeSet<String> {
        text.split('`')
            .skip(1)
            .step_by(2)
            .map(|token| match token.split_once('/') {
                Some((dir, _)) => format!("{dir}/"),
                None => token.to_string(),
            })
            .collect()
    }

    /// The project's own data records are CC0 (DATA-LICENSE.md). The paths it lists as CC0 are the
    /// records layer docs/architecture.md describes, no more and no fewer, so a records folder the
    /// plan adds is not left under the code's license, and DATA-LICENSE.md names no path the plan
    /// has dropped.
    #[test]
    fn the_data_license_covers_every_records_path() {
        let doc = fs::read_to_string(root().join("docs/architecture.md")).unwrap();
        let row = doc
            .lines()
            .find(|l| l.starts_with("| records |"))
            .expect("docs/architecture.md's Layers table has a records row");
        let layer = record_paths(row.split('|').nth(2).unwrap());
        let license = fs::read_to_string(root().join("DATA-LICENSE.md")).unwrap();
        let (_, listed) = license
            .split_once("\n## What is CC0\n")
            .expect("DATA-LICENSE.md has a section \"What is CC0\"");
        let listed = listed.split("\n## ").next().unwrap();
        let bullets: String = listed
            .lines()
            .filter(|l| l.starts_with("- "))
            .collect::<Vec<_>>()
            .join("\n");
        let licensed = record_paths(&bullets);
        assert_eq!(
            licensed, layer,
            "DATA-LICENSE.md's CC0 paths differ from the records layer in docs/architecture.md"
        );
        assert!(layer.len() >= 6, "found {layer:?}");
    }

    /// LICENSE-CC0 is CC0 1.0 Universal's legal code, byte for byte as
    /// creativecommons.org/publicdomain/zero/1.0/legalcode.txt served it on 2026-10-11 (the same
    /// text, whitespace aside, as GitHub's licenses API gives for cc0-1.0).
    #[test]
    fn license_cc0_is_the_cc0_legal_code() {
        use sha2::{Digest, Sha256};
        let bytes = fs::read(root().join("LICENSE-CC0")).unwrap();
        let hex: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex, "a2010f343487d3f7618affe54f789f5487602331c0a8d03f49e9a7c547cf0499",
            "LICENSE-CC0 is not the CC0 1.0 legal code it was copied from"
        );
    }

    /// Whether a dependency names exactly one version. None when its version is kept elsewhere: a
    /// path in this workspace, or [workspace.dependencies], which is checked in its own right.
    fn pinned(spec: &Value) -> Option<bool> {
        let exact = |v: Option<&str>| v.is_some_and(|v| v.starts_with('='));
        match spec {
            Value::String(v) => Some(exact(Some(v))),
            Value::Table(t) if t.contains_key("path") || t.contains_key("workspace") => None,
            Value::Table(t) => Some(exact(t.get("version").and_then(Value::as_str))),
            _ => Some(false),
        }
    }

    #[test]
    fn what_counts_as_pinned() {
        let spec = |s: &str| -> Value { s.parse::<Table>().unwrap().remove("d").unwrap() };
        assert_eq!(pinned(&spec("d = \"=1.2.3\"")), Some(true));
        assert_eq!(pinned(&spec("d = { version = \"=1.2.3\" }")), Some(true));
        assert_eq!(pinned(&spec("d = \"1.2.3\"")), Some(false));
        assert_eq!(pinned(&spec("d = { version = \"^1\" }")), Some(false));
        assert_eq!(pinned(&spec("d = { git = \"https://x\" }")), Some(false));
        assert_eq!(pinned(&spec("d = { path = \"../d\" }")), None);
        assert_eq!(pinned(&spec("d = { workspace = true }")), None);
    }

    /// Every dependency a manifest declares names one version (docs/architecture.md, "Rules").
    #[test]
    fn every_dependency_is_pinned() {
        let found = manifests();
        assert!(
            found.iter().any(|(at, _)| at == "crates/repo/Cargo.toml"),
            "found {:?}",
            found.iter().map(|(at, _)| at).collect::<Vec<_>>()
        );
        for (at, m) in found {
            for (name, spec) in dependencies(&m) {
                assert!(
                    pinned(&spec) != Some(false),
                    "{at}: {name} is not pinned to one version: {spec}"
                );
            }
        }
    }

    /// The names the crate in `dir` lists under [dependencies], sorted.
    fn runtime_dependencies(dir: &str) -> Vec<String> {
        let m = manifest(&root().join(dir).join("Cargo.toml"));
        let mut names: Vec<String> = m
            .get("dependencies")
            .and_then(Value::as_table)
            .map(|t| t.keys().cloned().collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    /// Reading the records is parsing and checking; the database and the network come after, in
    /// the warehouse, which fetches USDA's zips and loads them into DuckDB.
    #[test]
    fn parsing_records_needs_no_database_or_network() {
        assert_eq!(runtime_dependencies("crates/records"), ["serde", "toml"]);
        let warehouse = runtime_dependencies("crates/warehouse");
        for needed in ["balaena-records", "duckdb", "ureq"] {
            assert!(warehouse.iter().any(|d| d == needed), "{warehouse:?}");
        }
    }

    /// Readers read the export, not the warehouse: crates/core parses its JSON, so DuckDB is not in
    /// what reads it.
    #[test]
    fn reading_the_export_needs_no_database() {
        assert_eq!(runtime_dependencies("crates/core"), ["serde", "serde_json"]);
    }

    /// rust-toolchain.toml names one release, such as `1.96.0`: a channel like `stable` or
    /// `1.96` would move under every machine and CI without a commit.
    #[test]
    fn the_toolchain_is_one_release() {
        let toolchain = manifest(&root().join("rust-toolchain.toml"));
        let channel = toolchain["toolchain"]["channel"].as_str().unwrap();
        let parts: Vec<&str> = channel.split('.').collect();
        assert!(
            parts.len() == 3
                && parts
                    .iter()
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())),
            "rust-toolchain.toml's channel is {channel:?}, not one release"
        );
    }

    /// Every workflow in .github/workflows, by file name, with its text.
    fn workflows() -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = fs::read_dir(root().join(".github/workflows"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "yml"))
            .map(|p| {
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                (name, fs::read_to_string(&p).unwrap())
            })
            .collect();
        out.sort();
        out
    }

    fn quality_gate() -> String {
        workflows()
            .into_iter()
            .find(|(f, _)| f == "quality-gate.yml")
            .map(|(_, text)| text)
            .expect("the Quality Gate's workflow")
    }

    /// Every action a workflow uses is pinned to a commit, with the release it is in a comment: a
    /// tag can be moved, a commit cannot.
    #[test]
    fn every_action_is_pinned_by_commit() {
        let mut actions = 0;
        for (file, text) in workflows() {
            for line in text.lines() {
                let Some((_, used)) = line.split_once("uses: ") else {
                    continue;
                };
                actions += 1;
                let (action, rest) = used.split_once('@').unwrap_or((used, ""));
                let (sha, release) = rest.split_once(" # ").unwrap_or((rest, ""));
                let pinned = sha.len() == 40
                    && sha
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
                    && release.starts_with('v');
                assert!(pinned, "{file}: {action} is not pinned by commit: {line:?}");
            }
        }
        assert!(actions >= 2, "found {actions} actions");
    }

    /// Every cargo command in a workflow's text, as its subcommand and the words of its command,
    /// whatever comes before it on the line: a command after `&&`, `||`, `;` or `|` counts, and a
    /// toolchain (`+1.96.0`) or flag (`-q`) before the subcommand is skipped. Comment lines are not
    /// commands.
    fn cargo_commands(text: &str) -> Vec<(String, Vec<String>)> {
        let mut out = Vec::new();
        for line in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
            for command in line.split(['&', '|', ';']) {
                let words: Vec<String> = command.split_whitespace().map(String::from).collect();
                let Some(at) = words.iter().position(|w| w == "cargo") else {
                    continue;
                };
                let sub = words[at + 1..]
                    .iter()
                    .find(|w| !w.starts_with('+') && !w.starts_with('-'))
                    .cloned()
                    .unwrap_or_default();
                out.push((sub, words[at..].to_vec()));
            }
        }
        out
    }

    #[test]
    fn what_counts_as_a_cargo_command() {
        let subs = |text: &str| -> Vec<String> {
            cargo_commands(text).into_iter().map(|(s, _)| s).collect()
        };
        assert_eq!(subs("      - run: cargo test --locked"), ["test"]);
        assert_eq!(subs("run: cargo +1.96.0 -q build"), ["build"]);
        assert_eq!(
            subs("run: cargo fmt --check && cargo update"),
            ["fmt", "update"]
        );
        assert_eq!(
            subs("cargo doc; cargo run || cargo check | tee x"),
            ["doc", "run", "check"]
        );
        assert_eq!(
            subs("      # cargo test, in a comment"),
            Vec::<String>::new()
        );
        assert_eq!(subs("  ~/.cargo/registry/index"), Vec::<String>::new());
        assert_eq!(subs("  key: cargo-${{ runner.os }}"), Vec::<String>::new());
    }

    /// The versions a crate's manifest leaves open are pinned by Cargo.lock, but only where cargo
    /// is told to keep to it: every cargo command in the Quality Gate but `cargo fmt` runs with
    /// `--locked`, so a lockfile that is missing or out of date fails the gate instead of being
    /// written.
    #[test]
    fn the_gate_builds_with_the_lockfile() {
        let commands = cargo_commands(&quality_gate());
        for (sub, words) in &commands {
            assert!(
                sub == "fmt" || words.iter().any(|w| w == "--locked"),
                "the Quality Gate runs cargo {sub} without --locked: {:?}",
                words.join(" ")
            );
        }
        assert!(
            commands.len() >= 3,
            "found {} cargo commands",
            commands.len()
        );
    }

    /// A newer push to a pull request cancels the Quality Gate's older run on it; a run on main, or
    /// one started by hand, is a concurrency group of its own, so nothing cancels it or holds it
    /// back. Text only: that GitHub does so is seen on a pull request, not here.
    #[test]
    fn a_newer_push_cancels_only_a_pull_request_s_older_gate_run() {
        let gate = quality_gate();
        for needed in [
            "\nconcurrency:\n",
            "  group: quality-gate-${{ github.event.pull_request.number || github.run_id }}\n",
            "  cancel-in-progress: ${{ github.event_name == 'pull_request' }}\n",
        ] {
            assert!(gate.contains(needed), "the Quality Gate lacks {needed:?}");
        }
        assert_eq!(
            gate.matches("concurrency:").count(),
            1,
            "one concurrency block"
        );
        assert!(
            !gate.lines().any(|l| l.trim_start().starts_with("queue:")),
            "a queue would hold main's runs together"
        );
    }
}
