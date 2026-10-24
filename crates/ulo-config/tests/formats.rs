//! The file formats read into one tree (integrations DESIGN §2.1): TOML, JSON, YAML (behind the
//! `yaml` feature) and `.env` give the same typed value from the same document, and a value from
//! each carries its own line and column. JSON is strict RFC 8259.

mod common;

use serde::Deserialize;
use serde_json::json;
use ulo_config::{Config, DotEnv, Env, File, Section};

use common::{Dir, file_at, located, problems_of, text, tree};

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Typed {
    name: String,
    port: u16,
    ratio: f64,
    debug: bool,
    tags: Vec<String>,
    db: DbSection,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct DbSection {
    url: String,
    pool: u32,
    opts: Opts,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Opts {
    ssl: bool,
}

fn expected() -> Typed {
    Typed {
        name: "svc".into(),
        port: 8080,
        ratio: 0.5,
        debug: true,
        tags: vec!["a".into(), "b".into()],
        db: DbSection { url: "postgres://x".into(), pool: 4, opts: Opts { ssl: false } },
    }
}

const TOML: &str = "name = \"svc\"\nport = 8080\nratio = 0.5\ndebug = true\ntags = [\"a\", \"b\"]\n[db]\nurl = \"postgres://x\"\npool = 4\n[db.opts]\nssl = false\n";
const JSON: &str = r#"{
  "name": "svc", "port": 8080, "ratio": 0.5, "debug": true, "tags": ["a", "b"],
  "db": { "url": "postgres://x", "pool": 4, "opts": { "ssl": false } }
}"#;
#[cfg(feature = "yaml")]
const YAML: &str = "name: svc\nport: 8080\nratio: 0.5\ndebug: true\ntags: [a, b]\ndb:\n  url: postgres://x\n  pool: 4\n  opts:\n    ssl: false\n";
/// A `.env` file holds scalars only, so the list has no spelling there and the type it fills is
/// the scalar subset.
const DOTENV: &str = "NAME=svc\nPORT=8080\nRATIO=0.5\nDEBUG=true\nDB__URL=postgres://x\nDB__POOL=4\nDB__OPTS__SSL=false\n";

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Scalars {
    name: String,
    port: u16,
    ratio: f64,
    debug: bool,
    db: DbSection,
}

fn read<T: serde::de::DeserializeOwned + Clone>(config: &Config) -> T {
    match Section::<T>::read(config, "").value() {
        Ok(value) => value.clone(),
        Err(error) => panic!("the document reads: {error}"),
    }
}

#[test]
fn toml_and_json_read_to_the_same_tree_and_the_same_typed_value() {
    let dir = Dir::new();
    let toml = Config::builder().source(File::toml(dir.write("app.toml", TOML))).load();
    let json_config = Config::builder().source(File::json(dir.write("app.json", JSON))).load();
    assert_eq!(tree(&toml), tree(&json_config), "one tree whichever file format carried it");
    assert_eq!(tree(&toml)["ratio"], json!(0.5), "a float stays a float");
    assert_eq!(tree(&toml)["port"], json!(8080), "an integer stays an integer");
    assert_eq!(read::<Typed>(&toml), expected());
    assert_eq!(read::<Typed>(&json_config), expected());
}

#[cfg(feature = "yaml")]
#[test]
fn yaml_reads_to_the_same_tree_as_toml() {
    let dir = Dir::new();
    let toml = Config::builder().source(File::toml(dir.write("app.toml", TOML))).load();
    let yaml = Config::builder().source(File::yaml(dir.write("app.yaml", YAML))).load();
    assert_eq!(tree(&yaml), tree(&toml));
    assert_eq!(read::<Typed>(&yaml), expected());
}

/// `.env` and the environment have no types, and give the same typed value as a file does for
/// the scalar fields.
#[test]
fn dotenv_and_the_environment_read_to_the_same_typed_value_as_a_file() {
    let dir = Dir::new();
    let from_dotenv = Config::builder().source(DotEnv::file(dir.write(".env", DOTENV))).load();
    let vars = DOTENV.lines().filter_map(|line| line.split_once('='));
    let from_env = Config::builder().source(Env::from_map(vars)).load();
    let want = Scalars { name: "svc".into(), port: 8080, ratio: 0.5, debug: true, db: expected().db };
    assert_eq!(read::<Scalars>(&from_dotenv), want);
    assert_eq!(read::<Scalars>(&from_env), want);
}

/// An ill-typed value is located by the line and column of the value itself, in each format.
#[test]
fn a_bad_value_carries_its_line_and_column_in_each_format() {
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Port {
        port: u16,
    }
    let dir = Dir::new();
    let cases: Vec<(&str, File, &str, (u32, u32))> = vec![
        ("a.toml", File::toml(dir.path("a.toml")), "a = 1\nb = 2\nport = \"x\"\n", (3, 8)),
        ("a.json", File::json(dir.path("a.json")), "{\n  \"a\": 1,\n  \"port\": \"x\"\n}", (3, 11)),
        #[cfg(feature = "yaml")]
        ("a.yaml", File::yaml(dir.path("a.yaml")), "a: 1\nb: 2\nport: x\n", (3, 7)),
    ];
    for (name, source, body, (line, column)) in cases {
        dir.write(name, body);
        let config = Config::builder().source(source).load();
        let problems = problems_of::<Port>(&config, "");
        assert_eq!(problems.len(), 1, "{name}: {problems:?}");
        assert_eq!(problems[0].path(), "port", "{name}");
        assert_eq!(file_at(problems[0].origin().expect("an origin")), (text(&dir.path(name)), line, column), "{name}");
    }
}

/// A file that does not parse is a problem at the line of the fault, in each format, and a
/// source that failed supplies nothing.
#[test]
fn a_syntax_error_is_a_problem_at_its_line_in_each_format() {
    let dir = Dir::new();
    let cases: Vec<(&str, File, &str, u32)> = vec![
        ("bad.toml", File::toml(dir.path("bad.toml")), "a = 1\nb = = 2\n", 2),
        ("bad.json", File::json(dir.path("bad.json")), "{\n  \"a\": 1,\n  \"b\": ,\n}", 3),
        #[cfg(feature = "yaml")]
        ("bad.yaml", File::yaml(dir.path("bad.yaml")), "a: 1\nb: [1, 2\nc: 3\n", 3),
    ];
    for (name, source, body, line) in cases {
        dir.write(name, body);
        let config = Config::builder().source(source).load();
        let problems = located::<serde_json::Value>(&config, "");
        assert_eq!(problems.len(), 1, "{name}: {problems:?}");
        let (file, at, _) = file_at(problems[0].origin().expect("an origin"));
        assert_eq!((file, at), (text(&dir.path(name)), line), "{name}");
    }
}

/// `.json` is RFC 8259: no comments, no trailing comma, a key at most once, an object at the top.
#[test]
fn json_is_strict() {
    let dir = Dir::new();
    let cases: [(&str, &str, u32); 4] = [
        ("comment", "{\n  // no comments\n  \"a\": 1\n}", 2),
        ("trailing", "{\n  \"a\": 1,\n}", 2),
        ("duplicate", "{\n  \"a\": 1,\n  \"a\": 2\n}", 3),
        ("array", "[1, 2]", 1),
    ];
    for (name, body, line) in cases {
        let path = dir.write(&format!("{name}.json"), body);
        let config = Config::builder().source(File::json(&path)).load();
        let problems = located::<serde_json::Value>(&config, "");
        assert_eq!(problems.len(), 1, "{name}: {problems:?}");
        assert_eq!(file_at(problems[0].origin().expect("an origin")).1, line, "{name}");
    }
}

#[cfg(feature = "yaml")]
mod yaml {
    use super::*;

    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Flag {
        on: bool,
    }

    /// YAML 1.2's core schema: `yes` is a string, so a `bool` field refuses it as any other type
    /// problem, at the value's line; `true` is a boolean.
    #[test]
    fn scalars_resolve_by_the_1_2_core_schema() {
        let dir = Dir::new();
        let path = dir.write("flag.yaml", "on: yes\n");
        let config = Config::builder().source(File::yaml(&path)).load();
        let problems = problems_of::<Flag>(&config, "");
        assert_eq!(problems.len(), 1);
        assert_eq!(file_at(problems[0].origin().expect("an origin")), (text(&path), 1, 5));

        let path = dir.write("flag-true.yaml", "on: true\n");
        let config = Config::builder().source(File::yaml(&path)).load();
        assert!(Section::<Flag>::read(&config, "").value().expect("`true` is a boolean").on);
    }

    /// A merge key is YAML 1.1's: a problem at its line, never merged. A quoted `"<<"` is an
    /// ordinary key.
    #[test]
    fn a_merge_key_is_a_problem_naming_its_line_and_a_quoted_one_is_a_key() {
        let dir = Dir::new();
        let merged = dir.write("merge.yaml", "base: &b {x: 1}\nuse:\n  <<: *b\n  y: 2\n");
        let config = Config::builder().source(File::yaml(&merged)).load();
        let problems = located::<serde_json::Value>(&config, "");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(file_at(problems[0].origin().expect("an origin")), (text(&merged), 3, 3));

        let quoted = dir.write("quoted.yaml", "\"<<\": 1\n");
        let config = Config::builder().source(File::yaml(&quoted)).load();
        assert_eq!(tree(&config), json!({ "<<": 1 }));
    }

    /// An anchor and its alias within the bound are read, expanded in place.
    #[test]
    fn an_alias_within_the_bound_is_expanded() {
        let dir = Dir::new();
        let path = dir.write("alias.yaml", "base: &b {x: 1, y: [2, 3]}\ncopy: *b\n");
        let config = Config::builder().source(File::yaml(&path)).load();
        assert_eq!(tree(&config), json!({ "base": { "x": 1, "y": [2, 3] }, "copy": { "x": 1, "y": [2, 3] } }));
    }
}
