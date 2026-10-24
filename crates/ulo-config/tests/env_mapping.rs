//! How an environment variable name or a `.env` key becomes a path: the prefix removed, the rest
//! split at the separator and lowercased (integrations DESIGN §2.1, capability 2); and what reads
//! the process environment, which is only what the application names.

mod common;

use serde::Deserialize;
use serde_json::{Value, json};
use ulo_config::{Config, DotEnv, Env, Origin, Section, Values};

use common::{Dir, pairs, problems_of, sorted, tree};

/// The options of a row, the variables it sets, and the tree they produce.
struct Row {
    prefix: Option<&'static str>,
    separator: Option<&'static str>,
    vars: &'static [(&'static str, &'static str)],
    tree: fn() -> Value,
}

const ROWS: &[Row] = &[
    // The design's example: prefix `APP_`, the default separator `__`. A variable without the
    // prefix is left out, and so is one that is the prefix and nothing more.
    Row {
        prefix: Some("APP_"),
        separator: None,
        vars: &[("APP_HTTP__PORT", "80"), ("APP_DB__POOL__MAX", "5"), ("OTHER", "x"), ("APP_", "y")],
        tree: || json!({ "http": { "port": "80" }, "db": { "pool": { "max": "5" } } }),
    },
    // No prefix: the whole name is the path, lowercased; a single underscore is part of a name.
    Row {
        prefix: None,
        separator: None,
        vars: &[("HTTP__PORT", "80"), ("Mixed_Case", "1")],
        tree: || json!({ "http": { "port": "80" }, "mixed_case": "1" }),
    },
    // The default separator is two underscores, so a single one does not split.
    Row {
        prefix: Some("APP_"),
        separator: None,
        vars: &[("APP_HTTP_PORT", "80")],
        tree: || json!({ "http_port": "80" }),
    },
    // Another separator splits at it, and `__` then does not.
    Row {
        prefix: Some("APP_"),
        separator: Some("_"),
        vars: &[("APP_HTTP_PORT", "80"), ("APP_A__B", "1")],
        tree: || json!({ "http": { "port": "80" }, "a": { "b": "1" } }),
    },
    // An empty separator splits nothing: one segment, the whole name.
    Row {
        prefix: None,
        separator: Some(""),
        vars: &[("A__B", "1")],
        tree: || json!({ "a__b": "1" }),
    },
    // The prefix is matched as written, case included.
    Row {
        prefix: Some("APP_"),
        separator: None,
        vars: &[("app_http__port", "80"), ("APP_HTTP__PORT", "81")],
        tree: || json!({ "http": { "port": "81" } }),
    },
];

fn dotenv_text(vars: &[(&str, &str)]) -> String {
    vars.iter().map(|(name, value)| format!("{name}={value}\n")).collect()
}

#[test]
fn an_environment_name_becomes_a_path_by_prefix_and_separator() {
    for (index, row) in ROWS.iter().enumerate() {
        let mut env = Env::from_map(row.vars.iter().copied());
        if let Some(prefix) = row.prefix {
            env = env.prefix(prefix);
        }
        if let Some(separator) = row.separator {
            env = env.separator(separator);
        }
        let config = Config::builder().source(env).load();
        assert_eq!(tree(&config), (row.tree)(), "row {index}: {:?}", row.vars);
    }
}

/// A `.env` key maps as an environment name does, under the same two options.
#[test]
fn a_dotenv_key_becomes_a_path_the_way_an_environment_name_does() {
    let dir = Dir::new();
    for (index, row) in ROWS.iter().enumerate() {
        // A file cannot hold a name the parser refuses; every row's names are plain.
        let path = dir.write(&format!("row{index}.env"), &dotenv_text(row.vars));
        let mut source = DotEnv::file(path);
        if let Some(prefix) = row.prefix {
            source = source.prefix(prefix);
        }
        if let Some(separator) = row.separator {
            source = source.separator(separator);
        }
        let config = Config::builder().source(source).load();
        assert_eq!(tree(&config), (row.tree)(), "row {index}: {:?}", row.vars);
    }
}

/// A value from the environment is text; a typed read parses the number or boolean the field
/// asks for, and a value that is not one is a problem naming the variable.
#[test]
fn an_environment_value_is_text_a_typed_read_parses() {
    #[derive(Debug, Deserialize)]
    struct Typed {
        port: u16,
        debug: bool,
        ratio: f64,
        name: String,
    }
    let config = Config::builder()
        .source(Env::from_map([("APP_PORT", "8080"), ("APP_DEBUG", "true"), ("APP_RATIO", "0.25"), ("APP_NAME", "123")]).prefix("APP_"))
        .load();
    let typed = Section::<Typed>::read(&config, "");
    let typed = typed.value().expect("every field parses from its text");
    assert_eq!((typed.port, typed.debug, typed.ratio, typed.name.as_str()), (8080, true, 0.25, "123"));

    let config = Config::builder().source(Env::from_map([("APP_PORT", "70000"), ("APP_DEBUG", "maybe"), ("APP_RATIO", "0.25"), ("APP_NAME", "n")]).prefix("APP_")).load();
    let problems = problems_of::<Typed>(&config, "");
    assert_eq!(
        sorted(pairs(problems.iter())),
        sorted(vec![
            ("port".to_owned(), Some(Origin::Variable { name: "APP_PORT".into() })),
            ("debug".to_owned(), Some(Origin::Variable { name: "APP_DEBUG".into() })),
        ]),
        "an out-of-range number and a non-boolean, each naming its variable"
    );
}

/// Nothing reads the process environment unless the application names `Env::process()`: a source
/// list without it sees none of it, and one with it sees every variable, under the same mapping.
#[test]
fn the_process_environment_is_read_only_through_env_process() {
    let path = std::env::var("PATH").expect("the test runs with a PATH");

    let without = Config::builder().source(Values::new().set("x", 1)).source(Env::from_map([("Y", "2")])).load();
    assert!(tree(&without).get("path").is_none(), "a map and values never reach the process environment");

    let with = Config::builder().source(Env::process()).load();
    assert_eq!(tree(&with).get("path"), Some(&Value::String(path)), "`Env::process()` reads it, the name lowercased");

    let filtered = Config::builder().source(Values::new().set("keep", 1)).source(Env::process().prefix("ULO_CONFIG_NO_SUCH_PREFIX_")).load();
    assert_eq!(tree(&filtered), json!({ "keep": 1 }), "a prefix nothing carries adds nothing");
}

/// A `.env` file is read into the layer and never into the process environment.
#[test]
fn loading_a_dotenv_file_never_writes_the_process_environment() {
    let name = format!("ULO_CONFIG_DOTENV_PROBE_{}", std::process::id());
    assert!(std::env::var(&name).is_err(), "the probe name is free");
    let dir = Dir::new();
    let path = dir.write(".env", &format!("{name}=written\n"));
    let config = Config::builder().source(DotEnv::file(&path)).load();
    assert_eq!(tree(&config).get(name.to_lowercase()), Some(&json!("written")), "the layer holds it");
    assert!(std::env::var(&name).is_err(), "the process environment does not");
    assert!(!std::env::vars().any(|(key, _)| key == name));
}
