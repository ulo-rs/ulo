use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use ex_33_config::{builder, config};
use ex_support::{get, start};
use serde_json::{Value, json};
use ulo::{ConnectError, FailureReason, StartupError, WiringError};
use ulo_config::{ConfigError, Env, Origin, Values};
use ulo_runtime_tokio::Tokio;

const SENTINEL: &str = "hunter2-sentinel";

fn shipped() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("config")
}

/// A scratch directory, removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("ex-33-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&dir).expect("the scratch directory is created");
        Dir(dir)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let file = self.0.join(name);
        std::fs::write(&file, text).expect("the file is written");
        file
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One field comes from each layer: the defaults supply the timeout, the file the pool size, the
/// `.env` file the port, the environment the greeting and the explicit values the host.
#[tokio::test]
async fn each_field_reads_from_the_last_source_that_supplies_it() {
    let env = Env::from_map([("APP_GREETING", "hello from the environment"), ("UNRELATED", "x")]);
    let config = config(&shipped(), env, Values::new().set("http.host", "127.0.0.1"));
    let server = start(builder(config)).await;

    let reply = get(server.addr, "/settings").await;
    assert_eq!(reply.status, 200);
    let body: Value = serde_json::from_str(&reply.body).unwrap();
    assert_eq!(
        body,
        json!({
            "greeting": "hello from the environment",
            "host": "127.0.0.1",
            "port": 5000,
            "pool_size": 10,
            "timeout_secs": 30,
            "database_url": "Secret([redacted])",
        })
    );
    assert!(!reply.body.contains("file-secret"), "{}", reply.body);
    server.stop().await.unwrap();
}

fn wiring_problems(dir: &Dir) -> (String, Vec<(String, Option<Origin>)>) {
    let config = config(&dir.0, Env::from_map::<&str, &str>([]), Values::new());
    let Err(StartupError::Wiring(errors)) = builder(config).runtime(Tokio::current()).wire() else {
        panic!("a configuration with problems wired");
    };
    let failed: Vec<_> = errors
        .iter()
        .filter_map(|entry| match entry {
            WiringError::ValueFailed { error, .. } => Some(error),
            _ => None,
        })
        .collect();
    assert_eq!(failed.len(), 1, "one entry for the module that read the configuration: {errors}");
    let problems = failed[0].downcast_ref::<ConfigError>().expect("the structured list stays reachable").problems();
    (errors.to_string(), problems.iter().map(|p| (p.path().to_owned(), p.origin().cloned())).collect())
}

/// A file with three ill-typed values reports all three, each at its own line.
#[tokio::test]
async fn a_bad_file_reports_every_problem_with_its_file_and_line() {
    let dir = Dir::new();
    let file = dir.write("app.toml", "greeting = \"hi\"\n[http]\nhost = \"h\"\nport = \"eighty\"\n[database]\nurl = \"postgres://u@h/d\"\npool_size = -3\ntimeout_secs = \"soon\"\n");
    let at = |line, column| Some(Origin::File { path: file.to_string_lossy().as_ref().into(), line, column });
    let (report, mut problems) = wiring_problems(&dir);
    problems.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        problems,
        [
            ("database.pool_size".to_owned(), at(7, 13)),
            ("database.timeout_secs".to_owned(), at(8, 16)),
            ("http.port".to_owned(), at(4, 8)),
        ]
    );
    assert!(report.contains("3 problems") && report.contains("http.port at ") && report.contains("app.toml:4"), "{report}");
}

/// A URL the value type refuses is a problem at its line, and the report quotes nothing of it,
/// although the type's own message does.
#[tokio::test]
async fn a_refused_secret_is_reported_by_path_and_line_and_never_by_value() {
    let dir = Dir::new();
    let file = dir.write(
        "app.toml",
        &format!("greeting = \"hi\"\n[http]\nhost = \"h\"\nport = 80\n[database]\nurl = \"mysql://app:{SENTINEL}@db/app\"\npool_size = 3\n"),
    );
    let (report, problems) = wiring_problems(&dir);
    let origin = Some(Origin::File { path: file.to_string_lossy().as_ref().into(), line: 6, column: 7 });
    assert_eq!(problems, [("database.url".to_owned(), origin)]);
    assert!(!report.contains(SENTINEL) && !report.contains("mysql://"), "the secret reached the report: {report}");
}

/// A key a typo put in `.env` or the environment, which no section reads, refuses `connect`,
/// naming each key and the source that supplied it.
#[tokio::test]
async fn deny_unread_refuses_a_misspelled_key_naming_its_source() {
    let dir = Dir::new();
    std::fs::copy(shipped().join("app.toml"), dir.0.join("app.toml")).unwrap();
    let dotenv = dir.write(".env", "# a typo for APP_HTTP__PORT\nAPP_HTTP__PROT=5000\n");
    let config = config(&dir.0, Env::from_map([("APP_POOL", "4")]), Values::new());
    let wired = builder(config).runtime(Tokio::current()).wire().expect("the configuration reads");
    let Err(StartupError::Connect(ConnectError::Hook { reason: FailureReason::Errored(error), .. })) = wired.connect().await else {
        panic!("a configuration with keys nothing reads connected");
    };
    let mut found: Vec<_> = error
        .downcast_ref::<ConfigError>()
        .expect("the hook's own error stays reachable")
        .problems()
        .iter()
        .map(|p| (p.path().to_owned(), p.origin().cloned()))
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        found,
        [
            ("http.prot".to_owned(), Some(Origin::File { path: dotenv.to_string_lossy().as_ref().into(), line: 2, column: 0 })),
            ("pool".to_owned(), Some(Origin::Variable { name: "APP_POOL".into() })),
        ]
    );
}
