//! Top-level sections are read separately: a leaf type's own `Deserialize` error ends the section
//! it sits in, and every other section still reports (integrations DESIGN §2.2, decision 46).

mod common;

use serde::{Deserialize, Serialize};
use ulo::Secret;
use ulo_config::{Config, Defaults, Origin, Problem, Section, Values};

/// Refuses what is not `postgres://`, quoting its input as `Url` and `IpAddr` do.
#[derive(Clone, Debug, Deserialize)]
#[serde(try_from = "String")]
struct DbUrl(#[allow(dead_code)] String);

impl TryFrom<String> for DbUrl {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        if text.starts_with("postgres://") { Ok(DbUrl(text)) } else { Err(format!("`{text}` is not a postgres:// url")) }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
struct App {
    database: Database,
    http: Http,
}

#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
struct Database {
    url: Secret<DbUrl>,
    pool: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
struct Http {
    port: u16,
}

#[derive(Serialize)]
struct Fallbacks {
    database: DatabaseFallbacks,
    http: HttpFallbacks,
}

impl Default for Fallbacks {
    fn default() -> Self {
        Fallbacks { database: DatabaseFallbacks { pool: 5 }, http: HttpFallbacks { port: 80 } }
    }
}

#[derive(Serialize)]
struct DatabaseFallbacks {
    pool: u32,
}

#[derive(Serialize)]
struct HttpFallbacks {
    port: u16,
}

fn explicit(key: &str) -> Origin {
    Origin::Explicit { key: key.to_owned() }
}

fn summary(problems: &[Problem]) -> Vec<(String, Option<Origin>, String)> {
    problems.iter().map(|p| (p.path().to_owned(), p.origin().cloned(), p.message().to_owned())).collect()
}

fn read(values: Values) -> Vec<Problem> {
    let config = Config::builder().source(values).load();
    common::problems_of::<App>(&config, "")
}

#[test]
fn a_refused_leaf_and_an_ill_typed_value_in_another_section_are_both_reported() {
    let problems = read(Values::new().set("database.url", "mysql://u@h/d").set("database.pool", 5).set("http.port", "eighty"));
    assert_eq!(
        summary(&problems),
        [
            ("database.url".to_owned(), Some(explicit("database.url")), "invalid value, found a string".to_owned()),
            ("http.port".to_owned(), Some(explicit("http.port")), "expected u16, found a string".to_owned()),
        ]
    );
    assert!(problems.iter().all(|p| !p.flattened()));
}

#[test]
fn a_missing_leaf_is_one_problem_and_another_section_still_reports() {
    let config = Config::builder()
        .source(Defaults::of::<Fallbacks>())
        .source(Values::new().set("http.port", "eighty"))
        .load();
    let problems = common::problems_of::<App>(&config, "");
    assert_eq!(
        summary(&problems),
        [
            ("http.port".to_owned(), Some(explicit("http.port")), "expected u16, found a string".to_owned()),
            ("database.url".to_owned(), None, "missing required field".to_owned()),
        ]
    );
}

#[test]
fn a_missing_leaf_alone_is_exactly_one_problem() {
    let config = Config::builder()
        .source(Defaults::of::<Fallbacks>())
        .load();
    let problems = common::problems_of::<App>(&config, "");
    assert_eq!(summary(&problems), [("database.url".to_owned(), None, "missing required field".to_owned())]);
}

/// A section's own problems found before the leaf error stay in the report.
#[test]
fn problems_found_before_a_leaf_error_in_its_section_are_kept() {
    let problems = read(Values::new().set("database.pool", "many").set("database.url", "mysql://u@h/d").set("http.port", "eighty"));
    let paths: Vec<_> = problems.iter().map(|p| p.path().to_owned()).collect();
    assert_eq!(paths, ["database.pool", "database.url", "http.port"]);
}

#[test]
fn a_section_read_of_one_table_is_unaffected() {
    let config = Config::builder().source(Values::new().set("database.url", "mysql://x").set("database.pool", 1)).load();
    let read = Section::<Database>::read(&config, "database");
    assert_eq!(summary(&read.value().unwrap_err().problems().iter().cloned().collect::<Vec<_>>()).len(), 1);
}

/// A problem in a section read before the failed one is reported once, although every pass reads
/// that section again.
#[test]
fn a_problem_read_before_the_failed_section_is_reported_once() {
    let problems = read(Values::new().set("http.port", "eighty").set("database.url", "mysql://u@h/d").set("database.pool", 5));
    let paths: Vec<_> = problems.iter().map(|p| p.path().to_owned()).collect();
    assert_eq!(paths, ["database.url", "http.port"]);
}

/// A failed section's problem that quotes the text of a secret read elsewhere is scrubbed too.
#[test]
fn a_set_aside_problem_quoting_a_secret_is_scrubbed() {
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Holder {
        token: Secret<String>,
        database: PlainDatabase,
        http: Http,
    }
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct PlainDatabase {
        url: DbUrl,
    }
    let config = Config::builder()
        .source(Values::new().set("token", "s3cr3t-token").set("database.url", "s3cr3t-token").set("http.port", "eighty"))
        .load();
    let problems = common::problems_of::<Holder>(&config, "");
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems.iter().all(|p| !p.message().contains("s3cr3t-token")), "{problems:?}");
}

/// A leaf type that maps its input after reading it raises its refusal outside the deserializer
/// of the field; the absent field is still one problem.
#[test]
fn a_missing_leaf_without_a_secret_is_exactly_one_problem() {
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Plain {
        database: PlainDatabase,
    }
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct PlainDatabase {
        url: DbUrl,
    }
    let config = Config::builder().source(Values::new().set("database.pool", 5)).load();
    let problems = common::problems_of::<Plain>(&config, "");
    assert_eq!(summary(&problems), [("database.url".to_owned(), Some(explicit("database.pool")), "missing required field".to_owned())]);
}

/// The documented limit: a missing field of the type read is reported when it is declared before a
/// section that ended on a leaf-type error, and not when it is declared after.
#[test]
fn a_missing_field_declared_after_a_failed_section_is_the_documented_gap() {
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Before {
        greeting: String,
        database: Database,
    }
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct After {
        database: Database,
        greeting: String,
    }
    let config = Config::builder().source(Values::new().set("database.url", "mysql://u@h/d").set("database.pool", 5)).load();
    let paths = |problems: Vec<Problem>| problems.iter().map(|p| p.path().to_owned()).collect::<Vec<_>>();
    assert_eq!(paths(common::problems_of::<Before>(&config, "")), ["database.url", "greeting"]);
    assert_eq!(paths(common::problems_of::<After>(&config, "")), ["database.url"]);
}

/// A leaf type that maps its input after reading it, a `try_from` outside a `Secret`, is reported
/// at its own path and origin, not its section's.
#[test]
fn a_leaf_refusing_after_it_is_read_is_reported_at_its_own_path() {
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Plain {
        database: PlainDatabase,
        http: Http,
    }
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct PlainDatabase {
        pool: u32,
        url: DbUrl,
    }
    let config = Config::builder().source(Values::new().set("database.pool", 5).set("database.url", "mysql://u@h/d").set("http.port", "eighty")).load();
    let problems = common::problems_of::<Plain>(&config, "");
    assert_eq!(
        summary(&problems),
        [
            ("database.url".to_owned(), Some(explicit("database.url")), "`mysql://u@h/d` is not a postgres:// url".to_owned()),
            ("http.port".to_owned(), Some(explicit("http.port")), "expected u16, found a string".to_owned()),
        ]
    );
}

/// A top-level key the type refuses, under `deny_unknown_fields`, is set aside like a section:
/// every section still reports, whichever comes first.
#[test]
fn an_unknown_top_level_key_ends_no_other_section() {
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Strict {
        database: Database,
        http: Http,
    }
    let config = Config::builder()
        .source(Values::new().set("stray", 1).set("database.url", "mysql://u@h/d").set("database.pool", 5).set("http.port", "eighty"))
        .load();
    let paths: Vec<_> = common::problems_of::<Strict>(&config, "").iter().map(|p| p.path().to_owned()).collect();
    assert_eq!(paths, ["stray", "database.url", "http.port"]);
}

/// The placeholder for a missing field of a flattened struct is buffered content; the flattened
/// type's refusal of it is no second problem.
#[test]
fn a_missing_field_behind_flatten_is_exactly_one_problem() {
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Listen {
        port: u16,
        host: String,
    }
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Flat {
        name: String,
        #[serde(flatten)]
        listen: Listen,
    }
    let config = Config::builder().source(Values::new().set("name", "n").set("host", "h")).load();
    let problems = common::problems_of::<Flat>(&config, "");
    assert_eq!(summary(&problems), [("port".to_owned(), Some(explicit("name")), "missing required field".to_owned())]);
}

/// A field missing inside an internally tagged enum, read from buffered content, is reported at
/// its own path, and the section it sits in ends without ending the others.
#[test]
fn a_missing_field_of_a_tagged_enum_names_its_path() {
    #[derive(Debug, Deserialize)]
    #[serde(tag = "kind")]
    #[allow(dead_code)]
    enum Store {
        Postgres { url: String },
    }
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Tagged {
        store: Store,
        http: Http,
    }
    let config = Config::builder().source(Values::new().set("store.kind", "Postgres").set("http.port", "eighty")).load();
    let paths: Vec<_> = common::problems_of::<Tagged>(&config, "").iter().map(|p| p.path().to_owned()).collect();
    assert_eq!(paths, ["store.url", "http.port"]);
}
