//! What `wire()` refuses before any connection is tried: the report names the section's path and
//! quotes no part of the URL.

use ex_35_db_sqlx::{builder, config};
use ulo::StartupError;
use ulo_config::{Env, Values};
use ulo_runtime_tokio::Tokio;

fn refusal(values: Values) -> String {
    match builder(config(Env::from_map::<&str, &str>([]), values)).runtime(Tokio::current()).wire() {
        Err(StartupError::Wiring(errors)) => errors.to_string(),
        other => panic!("the configuration wired: {:?}", other.map(drop)),
    }
}

#[tokio::test]
async fn a_url_of_another_engine_is_refused_at_wire_without_echoing_it() {
    let report = refusal(Values::new().set("database.url", "mysql://app:hunter2-sentinel@db.internal/app"));
    assert!(report.contains("database.url") && report.contains("postgres://"), "{report}");
    assert!(!report.contains("hunter2-sentinel") && !report.contains("db.internal"), "{report}");
}

#[tokio::test]
async fn a_missing_url_is_refused_at_wire_naming_the_key() {
    let report = refusal(Values::new());
    assert!(report.contains("database.url"), "{report}");
}
