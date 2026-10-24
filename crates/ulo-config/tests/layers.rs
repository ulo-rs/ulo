//! Source layering, origin tracking and the aggregated report (integrations DESIGN §2.1, §2.2,
//! capabilities 1 to 3, decision 73; SWITCH_INVENTORY G16).
//!
//! What a layer supplied is read back through the typed section, and where a value came from is
//! read from the `Origin` a problem carries, so every origin assertion is on the variant and its
//! fields and none on message text.

mod common;

use serde::{Deserialize, Serialize};
use serde_json::json;
use ulo::{App, Module, ModuleDef, ModuleIdentity, StartupError, WiringError};
use ulo_config::{Config, ConfigError, ConfigModule, Defaults, DotEnv, Env, File, Origin, Problems, Section, Values};

use common::{Dir, file_at, pairs, problems_of, sorted, text, tree};

/// Five keys; the value each layer writes is the layer's own name, so what a read gives names the
/// layer that won.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct Layered {
    a: String,
    b: String,
    c: String,
    d: String,
    e: String,
}

impl Default for Layered {
    fn default() -> Self {
        let d = || String::from("default");
        Layered { a: d(), b: d(), c: d(), d: d(), e: d() }
    }
}

fn layered(config: &Config) -> [String; 5] {
    let read = Section::<Layered>::read(config, "");
    let Layered { a, b, c, d, e } = read.value().unwrap_or_else(|error| panic!("the layers read: {error}")).clone();
    [a, b, c, d, e]
}

/// Each layer covers the keys from `a` through its own depth: values `a`, env `a`-`b`, `.env`
/// `a`-`c`, file `a`-`d`, defaults all five.
struct Sources {
    file: std::path::PathBuf,
    dotenv: std::path::PathBuf,
}

fn sources(dir: &Dir) -> Sources {
    Sources {
        file: dir.write("app.toml", "a = \"file\"\nb = \"file\"\nc = \"file\"\nd = \"file\"\n"),
        dotenv: dir.write(".env", "APP_A=dotenv\nAPP_B=dotenv\nAPP_C=dotenv\n"),
    }
}

fn env() -> Env {
    Env::from_map([("APP_A", "env"), ("APP_B", "env")]).prefix("APP_")
}

fn values() -> Values {
    Values::new().set("a", "values")
}

/// The design's order, defaults then file then `.env` then environment then values, is the order
/// the sources were added in; each key reads from the last layer that supplies it.
#[test]
fn a_later_source_overrides_every_earlier_one() {
    let dir = Dir::new();
    let s = sources(&dir);
    let config = Config::builder()
        .source(Defaults::of::<Layered>())
        .source(File::toml(&s.file))
        .source(DotEnv::file(&s.dotenv).prefix("APP_"))
        .source(env())
        .source(values())
        .load();
    assert_eq!(layered(&config), ["values", "env", "dotenv", "file", "default"]);
}

/// The order is the order added, not a ranking of the kinds: the same five sources added in
/// another order give another answer.
#[test]
fn the_order_added_decides_and_not_the_kind_of_source() {
    let dir = Dir::new();
    let s = sources(&dir);
    let config = Config::builder()
        .source(values())
        .source(Defaults::of::<Layered>())
        .source(env())
        .source(DotEnv::file(&s.dotenv).prefix("APP_"))
        .source(File::toml(&s.file))
        .load();
    assert_eq!(layered(&config), ["file", "file", "file", "file", "default"]);

    let config = Config::builder().source(File::toml(&s.file)).source(Defaults::of::<Layered>()).load();
    assert_eq!(layered(&config), ["default"; 5], "defaults added after a file override it");
}

/// Tables merge key by key and a later layer's leaf replaces the earlier leaf; an array is a leaf
/// and is replaced whole, never merged element by element.
#[test]
fn tables_merge_by_key_and_an_array_is_replaced_whole() {
    let dir = Dir::new();
    let first = dir.write("base.toml", "[http]\nhost = \"h\"\nport = 80\nhosts = [\"a\", \"b\", \"c\"]\n[limits]\nrate = 5\n");
    let second = dir.write("over.json", "{\"http\": {\"port\": 81, \"hosts\": [\"x\"]}}");
    let config = Config::builder().source(File::toml(&first)).source(File::json(&second)).source(Values::new().set("limits.burst", 9)).load();
    assert_eq!(
        tree(&config),
        json!({
            "http": { "host": "h", "port": 81, "hosts": ["x"] },
            "limits": { "rate": 5, "burst": 9 },
        })
    );
}

/// A scalar from a later layer over a table, and a table over a scalar, each replace what was at
/// the path.
#[test]
fn a_leaf_replaces_a_table_and_a_table_replaces_a_leaf() {
    let dir = Dir::new();
    let first = dir.write("base.toml", "x = 1\n[y]\nk = 2\n");
    let second = dir.write("over.toml", "y = 3\n[x]\nk = 4\n");
    let config = Config::builder().source(File::toml(&first)).source(File::toml(&second)).load();
    assert_eq!(tree(&config), json!({ "x": { "k": 4 }, "y": 3 }));
}

/// One ill-typed value from each source, read as numbers: the problem names the layer by the
/// variant of its origin and the fields that locate the value in it.
#[test]
fn every_value_carries_the_origin_of_the_source_that_supplied_it() {
    #[derive(Deserialize, Serialize)]
    struct Seeded {
        from_default: String,
    }
    impl Default for Seeded {
        fn default() -> Self {
            Seeded { from_default: "not a number".into() }
        }
    }
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Numbers {
        from_default: u16,
        from_file: u16,
        from_dotenv: u16,
        from_env: u16,
        from_values: u16,
    }

    let dir = Dir::new();
    let file = dir.write("app.toml", "# a comment\n\nfrom_file = \"oops\"\n");
    let dotenv = dir.write(".env", "# a comment\nOTHER=1\nFROM_DOTENV=oops\n");
    let config = Config::builder()
        .source(Defaults::of::<Seeded>())
        .source(File::toml(&file))
        .source(DotEnv::file(&dotenv))
        .source(Env::from_map([("FROM_ENV", "oops")]))
        .source(Values::new().set("from_values", "oops"))
        .load();

    let problems = problems_of::<Numbers>(&config, "");
    let by_path = |path: &str| {
        let found: Vec<_> = problems.iter().filter(|problem| problem.path() == path).collect();
        assert_eq!(found.len(), 1, "exactly one problem at `{path}`: {problems:?}");
        found[0].origin().cloned().unwrap_or_else(|| panic!("the problem at `{path}` names no origin"))
    };
    assert_eq!(problems.len(), 5, "five ill-typed values, five problems: {problems:?}");
    assert_eq!(by_path("from_default"), Origin::Default);
    assert_eq!(file_at(&by_path("from_file")), (text(&file), 3, 13), "the value's own line and column in the TOML file");
    assert_eq!(file_at(&by_path("from_dotenv")), (text(&dotenv), 3, 0), "a `.env` value carries its line and no column");
    assert_eq!(by_path("from_env"), Origin::Variable { name: "FROM_ENV".into() });
    assert_eq!(by_path("from_values"), Origin::Explicit { key: "from_values".into() });
}

/// The origin of a value is the layer that won it, not one it overrode; a valid override clears
/// the problem the lower layer's value would have caused.
#[test]
fn the_origin_is_the_winning_layer_s_and_a_valid_override_clears_the_problem() {
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Port {
        port: u16,
    }
    let dir = Dir::new();
    let file = dir.write("app.toml", "port = \"file\"\n");

    let bad_file_only = Config::builder().source(File::toml(&file)).load();
    let problems = problems_of::<Port>(&bad_file_only, "");
    assert_eq!(file_at(problems[0].origin().expect("an origin")).0, text(&file));

    let both_bad = Config::builder().source(File::toml(&file)).source(Env::from_map([("PORT", "env")])).load();
    let problems = problems_of::<Port>(&both_bad, "");
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].origin(), Some(&Origin::Variable { name: "PORT".into() }), "the environment overrode the file, so the report names the variable");

    let overridden = Config::builder().source(File::toml(&file)).source(Values::new().set("port", 8080)).load();
    assert_eq!(Section::<Port>::read(&overridden, "").value().expect("a valid explicit value over a bad file value").port, 8080);
}

#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
struct Http {
    host: String,
    port: u16,
    hosts: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
struct Db {
    url: String,
    pool: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
struct Whole {
    http: Http,
    database: Db,
}

/// A section is read by its dotted path, and a problem inside it carries the path from the root of
/// the configuration, an array element by its index.
#[test]
fn a_section_is_read_by_path_and_its_problems_carry_the_full_path() {
    let dir = Dir::new();
    let file = dir.write("app.toml", "[database]\nurl = \"postgres://db\"\npool = 4\n[http]\nhost = \"h\"\nport = 80\nhosts = [\"a\", 2]\n");
    let config = Config::builder().source(File::toml(&file)).load();

    let db = Section::<Db>::read(&config, "database");
    assert_eq!(db.path(), "database");
    let db = db.value().expect("the database section reads without knowing the application's type");
    assert_eq!((db.url.as_str(), db.pool), ("postgres://db", 4));

    let problems = problems_of::<Http>(&config, "http");
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].path(), "http.hosts[1]");
    assert_eq!(file_at(problems[0].origin().expect("an origin")), (text(&file), 7, 15));

    let missing = problems_of::<Db>(&config, "nowhere");
    assert!(!missing.is_empty(), "a section that is not in the tree is refused");
    assert!(missing.iter().all(|problem| problem.path().starts_with("nowhere") || problem.path().is_empty()), "{missing:?}");
}

/// Every problem in the configuration arrives in one report: a missing field, three ill-typed
/// values from three sources and an unreadable file, each with its path and the origin that
/// supplied it. None is dropped after the first.
#[test]
fn every_problem_is_reported_together_each_with_its_path_and_origin() {
    let dir = Dir::new();
    let file = dir.write("app.toml", "[http]\nhost = \"h\"\nport = \"eighty\"\nhosts = []\n[database]\nurl = \"u\"\n");
    let absent = dir.path("absent.toml");
    let config = Config::builder()
        .source(File::toml(&file))
        .source(File::toml(&absent))
        .source(Env::from_map([("APP_DATABASE__POOL", "many")]).prefix("APP_"))
        .source(Values::new().set("http.hosts", 3))
        .load();

    let problems = problems_of::<Whole>(&config, "");
    let found = sorted(pairs(problems.iter()));
    let expected = sorted(vec![
        ("http.port".to_owned(), Some(Origin::File { path: file.to_string_lossy().as_ref().into(), line: 3, column: 8 })),
        ("database.pool".to_owned(), Some(Origin::Variable { name: "APP_DATABASE__POOL".into() })),
        ("http.hosts".to_owned(), Some(Origin::Explicit { key: "http.hosts".into() })),
        (String::new(), Some(Origin::File { path: absent.to_string_lossy().as_ref().into(), line: 0, column: 0 })),
    ]);
    assert_eq!(found, expected, "the report: {problems:?}");
    assert_eq!(problems.len(), expected.len(), "one problem per fault, none twice: {problems:?}");
}

/// A required file that is missing is a recorded problem naming the file, a missing `.optional()`
/// one supplies nothing and raises none, and a file that exists and does not parse is a problem
/// at the line of the fault. All three are reported beside the value problems.
#[test]
fn a_failed_source_is_recorded_and_reported_and_an_optional_one_is_not_a_failure() {
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Port {
        port: u16,
    }
    let dir = Dir::new();
    let broken = dir.write("broken.toml", "port = 1\nport = 2\n");
    let absent = dir.path("absent.toml");
    let config = Config::builder()
        .source(File::toml(dir.path("optional.toml")).optional())
        .source(File::toml(&absent))
        .source(File::toml(&broken))
        .load();

    let problems = problems_of::<Port>(&config, "");
    let (located, unlocated): (Vec<_>, Vec<_>) = problems.iter().partition(|problem| problem.origin().is_some());
    assert_eq!(unlocated.len(), 1, "the field no source supplied is the one problem with no origin: {problems:?}");
    assert_eq!(unlocated[0].path(), "port");
    let origins: Vec<(String, u32)> = located
        .iter()
        .map(|problem| {
            let (file, line, _) = file_at(problem.origin().expect("located"));
            (file, line)
        })
        .collect();
    assert!(origins.contains(&(text(&absent), 0)), "the missing required file, with no line: {problems:?}");
    assert!(origins.iter().any(|(file, line)| *file == text(&broken) && *line == 2), "the duplicate key at its line: {problems:?}");
    assert!(origins.iter().all(|(file, _)| *file != text(&dir.path("optional.toml"))), "the optional file raised none: {problems:?}");
    assert_eq!(located.len(), 2, "the missing file and the duplicate key, no more: {problems:?}");

    let only_optional = Config::builder().source(File::toml(dir.path("optional.toml")).optional()).source(Values::new().set("port", 1)).load();
    assert_eq!(Section::<Port>::read(&only_optional, "").value().expect("a missing optional file is not a failure").port, 1);
}

/// `Defaults::of::<T>()` supplies `T::default()` with the origin `default`, at the section its
/// `.at(path)` names and at the root when it names none; a layer above it takes the origin.
#[test]
fn defaults_of_a_type_carry_the_origin_default_at_the_path_they_are_placed() {
    #[derive(Deserialize, Serialize)]
    struct Limits {
        rate: String,
        burst: u32,
    }
    impl Default for Limits {
        fn default() -> Self {
            Limits { rate: "fast".into(), burst: 10 }
        }
    }
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Numeric {
        rate: u32,
        burst: u32,
    }

    let at_path = Config::builder().source(Defaults::of::<Limits>().at("http.limits")).load();
    assert_eq!(tree(&at_path), json!({ "http": { "limits": { "rate": "fast", "burst": 10 } } }));
    let problems = problems_of::<Numeric>(&at_path, "http.limits");
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].path(), "http.limits.rate");
    assert_eq!(problems[0].origin(), Some(&Origin::Default));

    let at_root = Config::builder().source(Defaults::of::<Limits>()).load();
    assert_eq!(tree(&at_root), json!({ "rate": "fast", "burst": 10 }));
    assert_eq!(problems_of::<Numeric>(&at_root, "")[0].origin(), Some(&Origin::Default));

    let overridden = Config::builder().source(Defaults::of::<Limits>()).source(Values::new().set("rate", "slow")).load();
    assert_eq!(problems_of::<Numeric>(&overridden, "")[0].origin(), Some(&Origin::Explicit { key: "rate".into() }));
}

struct Root(Config);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(ConfigModule::<Whole>::new(&self.0).validate(|whole: &Whole, p: &mut Problems| {
            if whole.http.port == 1 {
                p.at("http.port", "must not be a privileged port");
            }
            if whole.database.pool == 0 {
                p.at("database.pool", "must not be zero");
            }
        }));
    }
}

/// `wire()` reports the source failures, the value problems and the validation hook's findings in
/// one `ValueFailed` entry, and the structured list stays reachable through `Redacted`.
#[test]
fn wire_reports_source_value_and_validation_problems_in_one_entry() {
    let dir = Dir::new();
    let absent = dir.path("absent.toml");
    let config = Config::builder()
        .source(File::toml(&absent))
        .source(Values::new().set("http.host", "h").set("http.port", 1).set("http.hosts", "not a list").set("database.url", "u").set("database.pool", 0))
        .load();
    let Err(StartupError::Wiring(errors)) = App::builder(Root(config)).wire() else { panic!("a configuration with problems wired") };

    let failed: Vec<&ulo::Redacted> = errors
        .iter()
        .filter_map(|entry| match entry {
            WiringError::ValueFailed { error, .. } => Some(error),
            _ => None,
        })
        .collect();
    assert_eq!(failed.len(), 1, "one entry for the module that read the configuration: {errors}");
    let config_error = failed[0].downcast_ref::<ConfigError>().expect("the structured error stays reachable");
    let found = sorted(pairs(config_error.problems().iter()));
    let expected = sorted(vec![
        (String::new(), Some(Origin::File { path: absent.to_string_lossy().as_ref().into(), line: 0, column: 0 })),
        ("http.hosts".to_owned(), Some(Origin::Explicit { key: "http.hosts".into() })),
    ]);
    assert_eq!(found, expected, "the source failure and the ill-typed value are reported; the hook runs on a value that read");
    assert!(!errors.to_string().is_empty());
}

/// With the sources sound, the validation hook's findings are the whole report, each at the path
/// the hook named and with no origin of a value.
#[test]
fn a_validation_hook_s_findings_arrive_in_the_same_report() {
    let config = Config::builder()
        .source(
            Values::new()
                .set("http.host", "h")
                .set("http.port", 1)
                .set("database.url", "u")
                .set("database.pool", 0),
        )
        .load();
    #[derive(Clone, Debug, Deserialize)]
    struct Small {
        http: Http2,
        database: Db,
    }
    #[derive(Clone, Debug, Deserialize)]
    struct Http2 {
        port: u16,
    }
    struct SmallRoot(Config);
    impl Module for SmallRoot {
        fn identity(&self) -> ModuleIdentity {
            ModuleIdentity::of_type::<Self>()
        }
        fn register(&self, m: &mut ModuleDef<'_>) {
            m.import(ConfigModule::<Small>::new(&self.0).validate(|s: &Small, p: &mut Problems| {
                if s.http.port == 1 {
                    p.at("http.port", "must not be a privileged port");
                }
                if s.database.pool == 0 {
                    p.at("database.pool", "must not be zero");
                }
            }));
        }
    }
    let Err(StartupError::Wiring(errors)) = App::builder(SmallRoot(config)).wire() else { panic!("the hook's findings were not refused") };
    let failed: Vec<&ulo::Redacted> = errors
        .iter()
        .filter_map(|entry| match entry {
            WiringError::ValueFailed { error, .. } => Some(error),
            _ => None,
        })
        .collect();
    assert_eq!(failed.len(), 1);
    let problems = failed[0].downcast_ref::<ConfigError>().expect("a config error").problems();
    let paths: Vec<&str> = problems.iter().map(|p| p.path()).collect();
    assert_eq!(paths, ["http.port", "database.pool"], "both findings, in the order the hook wrote them");
    assert!(problems.iter().all(|p| p.origin().is_none()), "a hook's finding names no source");
}
