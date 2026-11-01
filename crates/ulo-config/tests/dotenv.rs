//! The `.env` subset the crate's own parser reads (integrations DESIGN §2.1, the table of
//! forms): `KEY=VALUE` with an optional `export ` prefix, single quotes literal, `\n` `\"` `\\`
//! in double quotes, `#` comments, no expansion, no multi-line values, a line number for every
//! value and every problem, and a key written twice a problem naming both lines.

mod common;

use serde::Deserialize;
use serde_json::json;
use ulo_config::{Config, DotEnv, Origin, Section};

use common::{Dir, file_at, located, problems_of, text, tree};

fn load(dir: &Dir, name: &str, body: &str) -> (Config, String) {
    let path = dir.write(name, body);
    (Config::builder().source(DotEnv::file(&path)).load(), text(&path))
}

/// One line of each form the table lists, each read to the value the table gives.
#[test]
fn each_form_the_design_lists_reads_to_its_value() {
    let dir = Dir::new();
    let body = [
        "# a comment line",
        "export A=1",
        "C = spaced",
        r"D='single \n $HOME # kept'",
        r#"E="double \"q\" \\ \n # kept""#,
        "F=bare # trailing comment",
        "G=has#hash",
        "H=$A",
        "I=",
        r#"J="quoted" # after the quote"#,
        "export_not=kept",
        "K=a b  c",
        "",
    ]
    .join("\n");
    let (config, _) = load(&dir, ".env", &body);
    assert_eq!(
        tree(&config),
        json!({
            "a": "1",
            "c": "spaced",
            "d": r"single \n $HOME # kept",
            "e": "double \"q\" \\ \n # kept",
            "f": "bare",
            "g": "has#hash",
            "h": "$A",
            "i": "",
            "j": "quoted",
            "export_not": "kept",
            "k": "a b  c",
        })
    );
}

/// Every value's origin is its own line, counting comments and blank lines, with no column.
#[test]
fn every_value_carries_its_line_number() {
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Numbers {
        first: u8,
        second: u8,
    }
    let dir = Dir::new();
    let (config, file) = load(&dir, ".env", "# comment\n\nexport FIRST=x\n# another\nSECOND=\"y\"\n");
    let problems = problems_of::<Numbers>(&config, "");
    assert_eq!(problems.len(), 2);
    let found: Vec<(&str, (String, u32, u32))> = problems.iter().map(|p| (p.path(), file_at(p.origin().expect("an origin")))).collect();
    assert_eq!(found, [("first", (file.clone(), 3, 0)), ("second", (file, 5, 0))]);
}

/// A malformed line is a problem at its own line, and every one in the file is reported
/// together; a file with one fails whole, so its sound lines supply nothing.
#[test]
fn every_malformed_line_is_reported_at_its_own_line() {
    let dir = Dir::new();
    let body = [
        "GOOD=1",                 // 1
        "no equals sign",         // 2: not KEY=VALUE
        "bad key=1",              // 3: a key may not hold a space
        "Q='open",                // 4: single quote not closed
        r#"R="bad \q escape""#,   // 5: escape other than \n \" \\
        r#"S="done" extra"#,      // 6: text after the closing quote
        "T=ok",                   // 7
        "",
    ]
    .join("\n");
    let (config, file) = load(&dir, ".env", &body);
    let problems = located::<serde_json::Value>(&config, "");
    let found: Vec<(String, (String, u32, u32))> = problems.iter().map(|p| (p.path().to_owned(), file_at(p.origin().expect("an origin")))).collect();
    let expected: Vec<(String, (String, u32, u32))> = [("", 2), ("", 3), ("Q", 4), ("R", 5), ("S", 6)].map(|(path, line)| (path.to_owned(), (file.clone(), line, 0))).to_vec();
    assert_eq!(found, expected, "the malformed lines, all of them, in file order");
    assert!(problems.iter().all(|p| p.path() != "GOOD" && p.path() != "T"), "sound lines raise no problem");
}

/// No value spans two lines: the first line of a quoted value left open is a problem, and the
/// continuation is a line of its own that is not `KEY=VALUE`.
#[test]
fn a_value_cannot_span_lines() {
    let dir = Dir::new();
    let (config, file) = load(&dir, ".env", "M=\"first\nsecond\"\n");
    let problems = located::<serde_json::Value>(&config, "");
    let found: Vec<(String, u32)> = problems.iter().map(|p| (p.path().to_owned(), file_at(p.origin().expect("an origin")).1)).collect();
    assert_eq!(found, [("M".to_owned(), 1), (String::new(), 2)]);
    assert!(problems.iter().all(|p| file_at(p.origin().expect("an origin")).0 == file));
}

/// A key written twice in one file is a problem at the second line naming the first as well.
#[test]
fn a_key_written_twice_is_a_problem_naming_both_lines() {
    let dir = Dir::new();
    let (config, file) = load(&dir, ".env", "A=1\nB=2\nA=3\n");
    let problems = located::<serde_json::Value>(&config, "");
    assert_eq!(problems.len(), 1, "one repeat, one problem: {problems:?}");
    assert_eq!(problems[0].path(), "A");
    assert_eq!(file_at(problems[0].origin().expect("an origin")), (file, 3, 0));
    // The design says the problem names both lines; the message is the one place the first is.
    let message = problems[0].message();
    assert!(message.contains('1') && message.contains('3'), "the message names lines 1 and 3: {message}");
}

/// A later source overriding a key is layering, not a repeat: the same key in two files raises
/// no problem and the later file's value is read.
#[test]
fn the_same_key_in_a_later_source_is_layering_and_not_a_repeat() {
    let dir = Dir::new();
    let first = dir.write("one.env", "A=1\n");
    let second = dir.write("two.env", "A=2\n");
    let config = Config::builder().source(DotEnv::file(&first)).source(DotEnv::file(&second)).load();
    assert_eq!(tree(&config), json!({ "a": "2" }));
    assert_eq!(Section::<serde_json::Value>::read(&config, "").value().map(|_| ()).map_err(|e| e.problems().len()), Ok(()));
}

/// A missing `.env` is a problem unless `.optional()`, which is how a deployment without one runs.
#[test]
fn a_missing_dotenv_file_is_a_failure_unless_optional() {
    let dir = Dir::new();
    let absent = dir.path(".env");
    let required = Config::builder().source(DotEnv::file(&absent)).load();
    let problems = located::<serde_json::Value>(&required, "");
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].origin(), Some(&Origin::File { path: text(&absent).as_str().into(), line: 0, column: 0 }));

    let optional = Config::builder().source(DotEnv::file(&absent).optional()).source(ulo_config::Values::new().set("k", "v")).load();
    assert_eq!(tree(&optional), json!({ "k": "v" }));
}
