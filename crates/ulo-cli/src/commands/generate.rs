use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use colored::Colorize;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::{is_keyword, render};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(clap::Args)]
pub struct GenerateArgs {
    /// What to generate
    #[arg(value_enum)]
    kind: Kind,

    /// The name, a Rust identifier in any case: `users`, `UserProfile`, `user_profile`
    name: String,

    /// The transport a controller serves; only `controller` takes it
    #[arg(long, value_enum)]
    transport: Option<ControllerTransport>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Kind {
    /// A module: `src/<name>_module.rs`
    Module,
    /// An injectable service: `src/<name>.rs`
    Provider,
    /// Handlers on a transport: `src/<name>_controller.rs`
    Controller,
    /// A WebSocket gateway: `src/<name>_gateway.rs`
    Gateway,
    /// A GraphQL root object: `src/<name>_resolver.rs`
    Resolver,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ControllerTransport {
    Http,
    Rpc,
}

/// A name in the spellings the templates need.
struct Names {
    pascal: String,
    snake: String,
    /// The snake name with `-` between its words, for an HTTP path.
    route: String,
    /// The Pascal name with its first letter lowered, GraphQL's field spelling.
    field: String,
}

/// What `ulo generate` wrote, and what its author does next.
struct Generated {
    file_stem: String,
    template: &'static str,
    /// The crates the file needs beyond a `ulo new` project's, as `Cargo.toml` lines.
    requires: Vec<String>,
    /// The lines that register it, to paste into the module that serves it.
    registration: Vec<String>,
}

pub async fn execute(args: GenerateArgs) -> Result<()> {
    if args.transport.is_some() && args.kind != Kind::Controller {
        bail!("`--transport` applies to `controller` alone");
    }
    let names = names(&args.name)?;
    let generated = generated(args.kind, args.transport, &names);
    if is_keyword(&generated.file_stem) {
        bail!("'{}' is a Rust keyword and cannot name a module", generated.file_stem);
    }
    let content = render(
        generated.template,
        &[
            ("pascal", names.pascal.as_str()),
            ("snake", names.snake.as_str()),
            ("route", names.route.as_str()),
            ("pattern", names.snake.as_str()),
            ("field", names.field.as_str()),
        ],
    )?;

    let here = std::env::current_dir().context("Failed to read the current directory")?;
    let Some(root) = project_root(&here) else {
        bail!("No project found: neither this directory nor any above it holds a `Cargo.toml` beside a `src` directory");
    };
    let path = root.join("src").join(format!("{}.rs", generated.file_stem));
    write_new(&path, &content).await?;
    // A manifest that cannot be read leaves every line printed, as if the project had none.
    let manifest = fs::read_to_string(root.join("Cargo.toml")).await.unwrap_or_default();
    let requires: Vec<&String> = generated.requires.iter().filter(|line| !declares(&manifest, line)).collect();

    let shown = path.strip_prefix(&here).unwrap_or(&path);
    println!("{}", format!("Created {}", shown.display()).green());
    println!("\nDeclare it where the project's modules are, usually src/main.rs:\n    mod {};", generated.file_stem);
    println!("\nRegister it in the module that serves it:");
    for line in &generated.registration {
        println!("    {line}");
    }
    if !requires.is_empty() {
        println!("\nIt needs these in Cargo.toml:");
        for line in requires {
            println!("    {line}");
        }
    }
    Ok(())
}

/// The nearest directory from `start` upward holding a `Cargo.toml` beside a `src` directory, which
/// is where `generate` writes however deep in the project it is run. A workspace's own root has no
/// `src`, so the search passes it and finds nothing rather than writing beside the members.
fn project_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|dir| dir.join("Cargo.toml").is_file() && dir.join("src").is_dir()).map(Path::to_path_buf)
}

/// Whether `manifest` already declares the crate a `Cargo.toml` line names, in any of the spellings
/// that declare one: `name = ..` or `name.workspace = true` under a dependencies table, or a
/// `[dependencies.name]` table of its own. A `[patch]` entry and a workspace's shared
/// `[workspace.dependencies]` do not make the member depend on it.
fn declares(manifest: &str, line: &str) -> bool {
    let Some(name) = line.split(|c: char| c.is_whitespace() || c == '=').next().filter(|name| !name.is_empty()) else {
        return false;
    };
    let mut in_dependencies = false;
    for text in manifest.lines().map(str::trim) {
        if let Some(header) = text.strip_prefix('[') {
            let table = header.trim_start_matches('[').trim_end_matches(']').trim();
            let own = !table.starts_with("workspace.");
            if own && table.ends_with(&format!("dependencies.{name}")) {
                return true;
            }
            in_dependencies = own && table.ends_with("dependencies");
        } else if in_dependencies {
            let key = text.strip_prefix(name).and_then(|rest| rest.chars().next());
            if matches!(key, Some(c) if c.is_whitespace() || c == '=' || c == '.') {
                return true;
            }
        }
    }
    false
}

/// The file's stem, template, requirements and registration for `kind`.
fn generated(kind: Kind, transport: Option<ControllerTransport>, names: &Names) -> Generated {
    let Names { pascal, snake, .. } = names;
    match kind {
        Kind::Module => Generated {
            file_stem: format!("{snake}_module"),
            template: include_str!("../templates/generate/module.rs"),
            requires: Vec::new(),
            registration: vec![format!("m.import(crate::{snake}_module::{pascal}Module);")],
        },
        Kind::Provider => Generated {
            file_stem: snake.clone(),
            template: include_str!("../templates/generate/provider.rs"),
            requires: Vec::new(),
            registration: vec![format!("m.provide::<crate::{snake}::{pascal}>();")],
        },
        Kind::Controller => {
            let (template, requires) = match transport.unwrap_or(ControllerTransport::Http) {
                ControllerTransport::Http => {
                    (include_str!("../templates/generate/controller_http.rs"), format!("ulo-http = \"={VERSION}\""))
                }
                ControllerTransport::Rpc => {
                    (include_str!("../templates/generate/controller_rpc.rs"), format!("ulo-rpc = \"={VERSION}\""))
                }
            };
            Generated {
                file_stem: format!("{snake}_controller"),
                template,
                requires: vec![requires],
                registration: vec![format!("m.controller::<crate::{snake}_controller::{pascal}Controller>();")],
            }
        }
        Kind::Gateway => Generated {
            file_stem: format!("{snake}_gateway"),
            template: include_str!("../templates/generate/gateway.rs"),
            requires: vec![format!("ulo-ws = \"={VERSION}\"")],
            // A gateway on the HTTP server's port is reachable only once `WsModule` is imported,
            // which is once per application and not once per gateway.
            registration: vec![
                format!("m.controller::<crate::{snake}_gateway::{pascal}Gateway>();"),
                "m.import(ulo_ws::WsModule::for_root()); // once per application".to_owned(),
            ],
        },
        Kind::Resolver => Generated {
            file_stem: format!("{snake}_resolver"),
            template: include_str!("../templates/generate/resolver.rs"),
            requires: vec!["async-graphql = \"7\"".to_owned()],
            registration: vec![format!(
                "// add crate::{snake}_resolver::{pascal}Resolver to the query root's #[derive(MergedObject)]"
            )],
        },
    }
}

/// Writes `content` to `path`, which must not exist: `create_new` refuses an existing file
/// atomically, so a file that appears between a check and the write is not overwritten either.
async fn write_new(path: &Path, content: &str) -> Result<()> {
    let mut file = match fs::OpenOptions::new().write(true).create_new(true).open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            bail!("{} already exists; `ulo generate` does not overwrite", path.display())
        }
        Err(error) => return Err(error).with_context(|| format!("Failed to create {}", path.display())),
    };
    let written = async {
        file.write_all(content.as_bytes()).await?;
        file.flush().await
    }
    .await;
    if let Err(error) = written {
        // A half-written file would refuse the next attempt as existing.
        drop(file);
        let _ = fs::remove_file(path).await;
        return Err(error).with_context(|| format!("Failed to write {}", path.display()));
    }
    Ok(())
}

/// The spellings of `name`, once it is known to be a name a Rust item can carry.
fn names(name: &str) -> Result<Names> {
    check_identifier(name)?;
    let snake = snake_case(name);
    let pascal = pascal_case(name);
    if !pascal.starts_with(|c: char| c.is_ascii_alphabetic()) {
        bail!("'{name}' must have a letter before any digit");
    }
    let route = snake.split('_').filter(|word| !word.is_empty()).collect::<Vec<_>>().join("-");
    let mut field = pascal.clone();
    field[..1].make_ascii_lowercase();
    Ok(Names { pascal, snake, route, field })
}

fn check_identifier(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        bail!("The name cannot be empty");
    };
    if !(first.is_ascii_alphabetic() || first == '_') || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        bail!("'{name}' is not an identifier: use letters, digits and '_', and do not start with a digit");
    }
    if !name.chars().any(|c| c.is_ascii_alphanumeric()) {
        bail!("'{name}' has no letter or digit in it");
    }
    if is_keyword(name) {
        bail!("'{name}' is a Rust keyword");
    }
    Ok(())
}

/// `UserProfile` and `user_profile` to `user_profile`, `HTTPServer` to `http_server`.
fn snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut snake = String::with_capacity(name.len() + 4);
    for (at, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() && at > 0 {
            let previous = chars[at - 1];
            let word_starts = previous.is_ascii_lowercase() || previous.is_ascii_digit();
            let acronym_ends =
                previous.is_ascii_uppercase() && chars.get(at + 1).is_some_and(|next| next.is_ascii_lowercase());
            if word_starts || acronym_ends {
                snake.push('_');
            }
        }
        snake.push(c.to_ascii_lowercase());
    }
    snake
}

/// `user_profile` and `userProfile` to `UserProfile`; an acronym keeps its capitals.
fn pascal_case(name: &str) -> String {
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{ControllerTransport, Kind, declares, generated, names, project_root, snake_case, write_new};
    use crate::commands::render;

    /// A fresh directory under the system's, removed by the test that made it.
    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ulo-generate-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory");
        dir.canonicalize().expect("its real path")
    }

    fn project(dir: &Path) {
        std::fs::create_dir_all(dir.join("src/nested")).expect("src");
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"app\"\n").expect("a manifest");
    }

    #[test]
    fn the_root_is_found_from_a_directory_below_it() {
        let dir = scratch("root-below");
        project(&dir);
        assert_eq!(project_root(&dir.join("src/nested")), Some(dir.clone()));
        assert_eq!(project_root(&dir), Some(dir.clone()));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn a_workspace_root_without_src_is_passed_over() {
        let dir = scratch("root-workspace");
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").expect("a manifest");
        project(&dir.join("app"));
        assert_eq!(project_root(&dir.join("app/src")), Some(dir.join("app")));
        assert_eq!(project_root(&dir), None, "the workspace's own root is no project");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn no_manifest_means_no_project() {
        let dir = scratch("root-none");
        std::fs::create_dir_all(dir.join("src")).expect("src");
        assert_eq!(project_root(&dir.join("src")), None);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn a_declared_crate_is_recognised_in_each_spelling() {
        let line = "ulo-http = \"=0.1.0\"";
        for manifest in [
            "[dependencies]\nulo-http = \"=0.1.0\"\n",
            "[dependencies]\nulo-http = { version = \"=0.1.0\" }\n",
            "[dependencies]\nulo-http.workspace = true\n",
            "[dependencies.ulo-http]\nversion = \"=0.1.0\"\n",
            "[dependencies]\n    ulo-http   = \"=0.1.0\"\n",
            "[dev-dependencies]\nulo-http = \"=0.1.0\"\n",
            "[target.'cfg(unix)'.dependencies]\nulo-http = \"=0.1.0\"\n",
        ] {
            assert!(declares(manifest, line), "not recognised in {manifest:?}");
        }
    }

    #[test]
    fn a_crate_sharing_a_prefix_a_name_value_or_a_patch_is_not_the_declared_one() {
        let line = "ulo-http = \"=0.1.0\"";
        for manifest in [
            "[dependencies]\nulo-http-hyper = \"=0.1.0\"\n",
            "[package]\nname = \"ulo-http\"\n",
            "[dependencies]\nulo = \"=0.1.0\"\n",
            "[patch.crates-io]\nulo-http = { path = \"../ulo-http\" }\n",
            "[workspace.dependencies]\nulo-http = \"=0.1.0\"\n",
            "[dependencies]\nulo = \"=0.1.0\"\n\n[features]\nulo-http = []\n",
            "",
        ] {
            assert!(!declares(manifest, line), "recognised in {manifest:?}");
        }
    }

    #[test]
    fn names_are_cased_for_each_place_they_appear() {
        let spelled = names("UserProfile").expect("a name");
        assert_eq!((spelled.pascal.as_str(), spelled.snake.as_str()), ("UserProfile", "user_profile"));
        assert_eq!((spelled.route.as_str(), spelled.field.as_str()), ("user-profile", "userProfile"));
        assert_eq!(snake_case("HTTPServer"), "http_server");
        assert_eq!(names("user_profile").expect("a name").pascal, "UserProfile");
    }

    #[test]
    fn a_name_that_is_not_an_identifier_is_refused() {
        for name in ["", "1abc", "a-b", "fn", "_", "9"] {
            assert!(names(name).is_err(), "{name:?} was accepted");
        }
    }

    #[test]
    fn every_kind_renders_with_no_slot_left() {
        let spelled = names("orders").expect("a name");
        let kinds = [
            (Kind::Module, None),
            (Kind::Provider, None),
            (Kind::Controller, Some(ControllerTransport::Http)),
            (Kind::Controller, Some(ControllerTransport::Rpc)),
            (Kind::Gateway, None),
            (Kind::Resolver, None),
        ];
        for (kind, transport) in kinds {
            let made = generated(kind, transport, &spelled);
            let content = render(
                made.template,
                &[
                    ("pascal", spelled.pascal.as_str()),
                    ("snake", spelled.snake.as_str()),
                    ("route", spelled.route.as_str()),
                    ("pattern", spelled.snake.as_str()),
                    ("field", spelled.field.as_str()),
                ],
            )
            .unwrap_or_else(|error| panic!("{kind:?}: {error}"));
            assert!(content.contains("Orders"), "{kind:?} wrote no name");
        }
    }

    #[tokio::test]
    async fn an_existing_file_is_not_overwritten() {
        let dir = scratch("overwrite");
        let path = dir.join("x.rs");
        write_new(&path, "first").await.expect("a new file");
        let error = write_new(&path, "second").await.expect_err("an existing file");
        assert!(error.to_string().contains("does not overwrite"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).expect("the file"), "first");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
