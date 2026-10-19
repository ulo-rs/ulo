//! Premise check R2: the pre-scan refuses an alias bomb before saphyr's loader expands it.
#![cfg(feature = "yaml")]

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use ulo_config::{Config, File, Section};

/// Billion-laughs: level 0 holds 10 scalars, each next level holds 10 aliases of the one before.
/// Expanded, the last level is 10^levels nodes.
fn bomb(levels: usize) -> String {
    let mut text = String::from("l0: &l0 [a, a, a, a, a, a, a, a, a, a]\n");
    for level in 1..levels {
        let previous = level - 1;
        let refs = vec![format!("*l{previous}"); 10].join(", ");
        let _ = writeln!(text, "l{level}: &l{level} [{refs}]");
    }
    text
}

fn write(name: &str, text: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ulo-config-r2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

#[derive(Debug, Deserialize)]
struct Any {
    #[serde(flatten)]
    #[allow(dead_code)]
    rest: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Loads on a worker thread with a deadline; a hang aborts the process instead of the machine.
fn load_with_deadline(path: PathBuf, deadline: Duration) -> (String, Duration) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let started = Instant::now();
        let config = Config::builder().source(File::yaml(path)).load();
        let section = Section::<Any>::read(&config, "");
        let rendered = match section.value() {
            Ok(_) => "OK".to_owned(),
            Err(error) => error.to_string(),
        };
        let _ = tx.send((rendered, started.elapsed()));
    });
    match rx.recv_timeout(deadline) {
        Ok(done) => done,
        Err(_) => {
            eprintln!("alias bomb was not refused within {deadline:?}; aborting");
            std::process::abort();
        }
    }
}

#[test]
fn nested_bomb_is_refused_with_a_line_in_bounded_time() {
    // 10 levels x 10 refs would expand to 10^10 nodes.
    let path = write("bomb.yaml", &bomb(10));
    let (rendered, took) = load_with_deadline(path, Duration::from_secs(5));
    eprintln!("bomb refused in {took:?}: {rendered}");
    assert!(took < Duration::from_secs(2), "took {took:?}");
    assert!(rendered.contains("alias expands the document past 100000 nodes"), "{rendered}");
    assert!(rendered.contains("bomb.yaml:5:"), "the problem must name the line of the alias that passed the limit: {rendered}");
}

#[test]
fn legitimate_aliases_below_the_limit_load() {
    let text = "base: &b {host: db, port: 5432}\nprimary: *b\nreplica: *b\nlist: &l [1, 2, 3]\nagain: *l\n";
    let path = write("ok.yaml", text);
    let (rendered, took) = load_with_deadline(path, Duration::from_secs(5));
    assert_eq!(rendered, "OK", "{rendered}");
    assert!(took < Duration::from_secs(2));
}

#[test]
fn an_expansion_just_under_the_limit_loads() {
    // 4 levels of 10 = 10^4 expanded nodes plus the scaffolding: far below 100,000.
    let path = write("under.yaml", &bomb(4));
    let (rendered, _) = load_with_deadline(path, Duration::from_secs(5));
    assert_eq!(rendered, "OK", "{rendered}");
}

#[test]
fn merge_keys_are_refused_with_the_line() {
    let text = "base: &b {host: db}\nprimary:\n  <<: *b\n  port: 1\n";
    let path = write("merge.yaml", text);
    let (rendered, _) = load_with_deadline(path, Duration::from_secs(5));
    assert!(rendered.contains("merge key `<<` is YAML 1.1 and is not supported"), "{rendered}");
    assert!(rendered.contains("merge.yaml:3:"), "{rendered}");
}

#[test]
fn a_quoted_merge_key_is_an_ordinary_string_key() {
    let path = write("quoted.yaml", "primary:\n  \"<<\": 1\n");
    let (rendered, _) = load_with_deadline(path, Duration::from_secs(5));
    assert_eq!(rendered, "OK", "{rendered}");
}
