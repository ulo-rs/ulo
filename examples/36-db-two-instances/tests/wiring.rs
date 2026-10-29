//! Both sections are read at registration, so one `wire()` reports the problems of both.

use ex_36_db_two_instances::{builder, config};
use ulo::StartupError;
use ulo_config::{Env, Values};
use ulo_runtime_tokio::Tokio;

#[tokio::test]
async fn one_report_names_the_url_of_each_section_that_is_wrong() {
    let values = Values::new().set("primary.url", "mysql://app:hunter2-sentinel@db/app");
    let Err(StartupError::Wiring(errors)) = builder(config(Env::from_map::<&str, &str>([]), values)).runtime(Tokio::current()).wire() else {
        panic!("a configuration with two bad sections wired");
    };
    let report = errors.to_string();
    assert!(report.contains("primary.url") && report.contains("replica.url"), "{report}");
    assert!(!report.contains("hunter2-sentinel"), "{report}");
}
