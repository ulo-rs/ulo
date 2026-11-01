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

    let source = PathBuf::from("src");
    if !fs::try_exists(&source).await.unwrap_or(false) {
        bail!("There is no `src` directory here; run `ulo generate` from the project's root");
    }
    let path = source.join(format!("{}.rs", generated.file_stem));
    write_new(&path, &content).await?;

    println!("{}", format!("Created {}", path.display()).green());
    println!("\nDeclare it where the project's modules are, usually src/main.rs:\n    mod {};", generated.file_stem);
    println!("\nRegister it in the module that serves it:");
    for line in &generated.registration {
        println!("    {line}");
    }
    if !generated.requires.is_empty() {
        println!("\nIt needs these in Cargo.toml:");
        for line in &generated.requires {
            println!("    {line}");
        }
    }
    Ok(())
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
