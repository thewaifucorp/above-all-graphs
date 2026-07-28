//! Import aliases declared by a repository's own build manifests.
//!
//! Module resolution by path convention (`crate::bindings`) covers what a
//! language's syntax implies, but a repo can rename its own module space:
//! `tsconfig.json` maps `@app/*` onto `src/app/*`, a workspace package is
//! imported by its `package.json` name rather than its directory, and every
//! Go import is written against the `module` line in `go.mod`. Without
//! reading those files, each of those imports looks external and produces no
//! edge at all.
//!
//! Only declarations already in the repository are read — no toolchain is
//! invoked, no lockfile is resolved, nothing is fetched.

/// One alias: a pattern as written at import sites, and the repository paths
/// it stands for. A `*` in the pattern matches any suffix and is substituted
/// into each target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    /// Import specifier pattern (`@app/*`, `github.com/acme/app/*`).
    pub pattern: String,
    /// Repository-relative replacements, in preference order.
    pub targets: Vec<String>,
}

/// Every alias declared by the manifests found in one repository.
#[derive(Debug, Default, Clone)]
pub struct Toolchain {
    aliases: Vec<Alias>,
}

impl Toolchain {
    /// Collects aliases, longest pattern first so `@app/ui/*` wins over
    /// `@app/*`.
    #[must_use]
    pub fn new<I: IntoIterator<Item = Alias>>(aliases: I) -> Self {
        let mut aliases: Vec<Alias> = aliases.into_iter().collect();
        aliases.sort_by(|left, right| {
            right
                .pattern
                .len()
                .cmp(&left.pattern.len())
                .then_with(|| left.pattern.cmp(&right.pattern))
        });
        aliases.dedup();
        Self { aliases }
    }

    /// Repository-relative paths `source` could mean, in preference order.
    /// Empty when no manifest claims the specifier.
    #[must_use]
    pub fn expand(&self, source: &str) -> Vec<String> {
        let mut expanded = Vec::new();
        for alias in &self.aliases {
            match alias.pattern.split_once('*') {
                None => {
                    if alias.pattern == source {
                        expanded.extend(alias.targets.iter().cloned());
                    }
                }
                Some((prefix, suffix)) => {
                    if let Some(rest) = source
                        .strip_prefix(prefix)
                        .and_then(|rest| rest.strip_suffix(suffix))
                    {
                        expanded.extend(
                            alias
                                .targets
                                .iter()
                                .map(|target| target.replacen('*', rest, 1)),
                        );
                    }
                }
            }
        }
        expanded
    }

    /// Whether any manifest was read at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.aliases.is_empty()
    }
}

/// Manifest file names worth reading for alias declarations.
#[must_use]
pub fn is_manifest(relative_path: &str) -> bool {
    matches!(
        relative_path.rsplit('/').next().unwrap_or(relative_path),
        "tsconfig.json" | "jsconfig.json" | "package.json" | "go.mod"
    )
}

/// Aliases declared by one manifest, already rewritten to repository-relative
/// paths. An unparseable manifest yields nothing rather than failing the
/// index — a broken `tsconfig.json` must not stop the repo from being
/// indexed.
#[must_use]
pub fn manifest_aliases(relative_path: &str, contents: &str) -> Vec<Alias> {
    let directory = relative_path
        .rsplit_once('/')
        .map_or("", |(directory, _)| directory);
    match relative_path.rsplit('/').next().unwrap_or(relative_path) {
        "tsconfig.json" | "jsconfig.json" => typescript_aliases(directory, contents),
        "package.json" => package_aliases(directory, contents),
        "go.mod" => go_module_aliases(directory, contents),
        _ => Vec::new(),
    }
}

/// Joins a manifest-relative path onto the manifest's own directory,
/// collapsing `./` and `../`.
fn join(directory: &str, path: &str) -> String {
    let mut segments: Vec<&str> = if directory.is_empty() {
        Vec::new()
    } else {
        directory.split('/').collect()
    };
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

fn typescript_aliases(directory: &str, contents: &str) -> Vec<Alias> {
    let Ok(config) = serde_json::from_str::<serde_json::Value>(&strip_json_comments(contents))
    else {
        return Vec::new();
    };
    let options = &config["compilerOptions"];
    let base_url = options["baseUrl"].as_str().unwrap_or(".");
    let base = join(directory, base_url);
    let mut aliases = Vec::new();
    if let Some(paths) = options["paths"].as_object() {
        for (pattern, targets) in paths {
            let targets: Vec<String> = targets
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|target| target.as_str())
                .map(|target| join(&base, target))
                .collect();
            if !targets.is_empty() {
                aliases.push(Alias {
                    pattern: pattern.clone(),
                    targets,
                });
            }
        }
    }
    // A bare `baseUrl` also makes every path under it importable absolutely.
    if options["baseUrl"].is_string() {
        aliases.push(Alias {
            pattern: "*".to_string(),
            targets: vec![join(&base, "*")],
        });
    }
    aliases
}

fn package_aliases(directory: &str, contents: &str) -> Vec<Alias> {
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(contents) else {
        return Vec::new();
    };
    let mut aliases = Vec::new();
    // A workspace package is imported by name from its siblings.
    if let Some(name) = manifest["name"].as_str().filter(|name| !name.is_empty()) {
        let mut targets = Vec::new();
        for field in ["main", "module", "source"] {
            if let Some(entry) = manifest[field].as_str() {
                targets.push(join(directory, entry));
            }
        }
        targets.push(directory.to_string());
        aliases.push(Alias {
            pattern: name.to_string(),
            targets,
        });
        aliases.push(Alias {
            pattern: format!("{name}/*"),
            targets: vec![join(directory, "*")],
        });
    }
    // Node subpath imports (`#internal/db`).
    if let Some(imports) = manifest["imports"].as_object() {
        for (pattern, target) in imports {
            let target = match target {
                serde_json::Value::String(value) => Some(value.clone()),
                serde_json::Value::Object(map) => map
                    .values()
                    .find_map(|value| value.as_str())
                    .map(str::to_string),
                _ => None,
            };
            if let Some(target) = target {
                aliases.push(Alias {
                    pattern: pattern.clone(),
                    targets: vec![join(directory, &target)],
                });
            }
        }
    }
    aliases
}

fn go_module_aliases(directory: &str, contents: &str) -> Vec<Alias> {
    let Some(module) = contents
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("module "))
        .map(str::trim)
        .filter(|module| !module.is_empty())
    else {
        return Vec::new();
    };
    let base = if directory.is_empty() {
        String::new()
    } else {
        directory.to_string()
    };
    vec![
        Alias {
            pattern: module.to_string(),
            targets: vec![base.clone()],
        },
        Alias {
            pattern: format!("{module}/*"),
            targets: vec![if base.is_empty() {
                "*".to_string()
            } else {
                format!("{base}/*")
            }],
        },
    ]
}

/// Strips `//` and `/* */` comments and trailing commas so a commented
/// `tsconfig.json` — the norm, not the exception — still parses.
fn strip_json_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(character) = chars.next() {
        if in_string {
            out.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => {
                in_string = true;
                out.push(character);
            }
            '/' if chars.peek() == Some(&'/') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = '\0';
                for next in chars.by_ref() {
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
            }
            _ => out.push(character),
        }
    }
    strip_trailing_commas(&out)
}

fn strip_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut pending: Option<usize> = None;
    for character in text.chars() {
        if in_string {
            out.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => {
                pending = None;
                in_string = true;
                out.push(character);
            }
            ',' => {
                pending = Some(out.len());
                out.push(character);
            }
            ']' | '}' => {
                if let Some(index) = pending.take() {
                    out.remove(index);
                }
                out.push(character);
            }
            character if character.is_whitespace() => out.push(character),
            _ => {
                pending = None;
                out.push(character);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_tsconfig_path_alias() {
        let aliases = manifest_aliases(
            "tsconfig.json",
            r#"{"compilerOptions": {"baseUrl": ".", "paths": {"@app/*": ["src/app/*"]}}}"#,
        );
        let toolchain = Toolchain::new(aliases);
        assert_eq!(
            toolchain.expand("@app/widgets/list")[0],
            "src/app/widgets/list"
        );
    }

    #[test]
    fn tsconfig_base_url_makes_paths_absolute() {
        let toolchain = Toolchain::new(manifest_aliases(
            "tsconfig.json",
            r#"{"compilerOptions": {"baseUrl": "src"}}"#,
        ));
        assert_eq!(toolchain.expand("lib/format"), ["src/lib/format"]);
    }

    #[test]
    fn tsconfig_with_comments_and_trailing_commas_still_parses() {
        let toolchain = Toolchain::new(manifest_aliases(
            "tsconfig.json",
            "{\n  // project config\n  \"compilerOptions\": {\n    \"baseUrl\": \".\",\n    /* aliases */\n    \"paths\": {\n      \"@lib/*\": [\"packages/lib/*\"],\n    },\n  },\n}\n",
        ));
        assert_eq!(toolchain.expand("@lib/date")[0], "packages/lib/date");
    }

    #[test]
    fn nested_tsconfig_resolves_against_its_own_directory() {
        let toolchain = Toolchain::new(manifest_aliases(
            "apps/web/tsconfig.json",
            r#"{"compilerOptions": {"baseUrl": ".", "paths": {"~/*": ["src/*"]}}}"#,
        ));
        assert_eq!(toolchain.expand("~/app")[0], "apps/web/src/app");
    }

    #[test]
    fn workspace_package_name_maps_to_its_directory() {
        let toolchain = Toolchain::new(manifest_aliases(
            "packages/utils/package.json",
            r#"{"name": "@acme/utils", "main": "dist/index.js"}"#,
        ));
        assert_eq!(
            toolchain.expand("@acme/utils"),
            ["packages/utils/dist/index.js", "packages/utils"]
        );
        assert_eq!(
            toolchain.expand("@acme/utils/format"),
            ["packages/utils/format"]
        );
    }

    #[test]
    fn node_subpath_import_maps_to_its_target() {
        let toolchain = Toolchain::new(manifest_aliases(
            "package.json",
            r##"{"name": "app", "imports": {"#db/*": "./src/db/*"}}"##,
        ));
        assert_eq!(toolchain.expand("#db/pool"), ["src/db/pool"]);
    }

    #[test]
    fn go_module_prefix_maps_onto_the_module_root() {
        let toolchain = Toolchain::new(manifest_aliases(
            "go.mod",
            "module github.com/acme/app\n\ngo 1.22\n",
        ));
        assert_eq!(
            toolchain.expand("github.com/acme/app/internal/store"),
            ["internal/store"]
        );
    }

    #[test]
    fn nested_go_module_prefix_keeps_its_directory() {
        let toolchain = Toolchain::new(manifest_aliases(
            "services/api/go.mod",
            "module github.com/acme/api\n",
        ));
        assert_eq!(
            toolchain.expand("github.com/acme/api/store"),
            ["services/api/store"]
        );
    }

    #[test]
    fn unrelated_specifier_expands_to_nothing() {
        let toolchain = Toolchain::new(manifest_aliases(
            "tsconfig.json",
            r#"{"compilerOptions": {"paths": {"@app/*": ["src/app/*"]}}}"#,
        ));
        assert!(toolchain.expand("react").is_empty());
    }

    #[test]
    fn broken_manifest_is_skipped_not_fatal() {
        assert!(manifest_aliases("tsconfig.json", "{not json at all").is_empty());
        assert!(manifest_aliases("go.mod", "// no module line\n").is_empty());
    }

    #[test]
    fn longer_patterns_win_over_shorter_ones() {
        let toolchain = Toolchain::new(vec![
            Alias {
                pattern: "@app/*".into(),
                targets: vec!["src/*".into()],
            },
            Alias {
                pattern: "@app/ui/*".into(),
                targets: vec!["packages/ui/*".into()],
            },
        ]);
        assert_eq!(
            toolchain.expand("@app/ui/button"),
            ["packages/ui/button", "src/ui/button"]
        );
    }
}
