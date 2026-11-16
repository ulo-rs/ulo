//! `ulo::panic::install_redacting_hook`: what the process prints for a panic, with the hook and
//! without it. A panic hook is the process's, so each scenario runs in a child process, this test
//! binary run again on [`child`] alone with the scenario named in [`SCENARIO`], and the test reads
//! the child's stderr.

#[macro_use]
mod common;

use std::process::Command;

use common::*;
use ulo::Secret;

const SCENARIO: &str = "ULO_PANIC_HOOK_SCENARIO";

const SECRET: &str = "hook-token-7781";
const PASSWORD: &str = "pg-pass-3390";

struct Root;

/// One scenario, run when [`SCENARIO`] names it and passing at once otherwise: wires an app
/// registering [`SECRET`] unless the scenario is `unwired`, installs the hook unless it is
/// `default`, and panics on a thread of its own with a message carrying the secret and a url
/// holding [`PASSWORD`].
#[test]
fn child() {
    let Ok(scenario) = std::env::var(SCENARIO) else { return };
    if scenario == "hook_after_wire" {
        let app = wire_result(Mod::<Root>::new(|m| m.secret(&Secret::new(SECRET.to_owned())))).map_err(Box::new).expect("the app wires");
        ulo::panic::install_redacting_hook();
        panic_on_a_thread();
        drop(app);
        return;
    }
    if scenario != "default" {
        ulo::panic::install_redacting_hook();
    }
    let app = (scenario != "unwired")
        .then(|| wire_result(Mod::<Root>::new(|m| m.secret(&Secret::new(SECRET.to_owned())))).map_err(Box::new).expect("the app wires"));
    if scenario == "dropped" {
        drop(app);
        panic_on_a_thread();
    } else {
        panic_on_a_thread();
        drop(app);
    }
}

fn panic_on_a_thread() {
    let joined = std::thread::Builder::new()
        .name("panicking".to_owned())
        .spawn(|| panic!("connecting with {SECRET} to postgres://app:{PASSWORD}@db/main"))
        .expect("the thread starts")
        .join();
    assert!(joined.is_err(), "the thread panicked");
}

/// The child's stderr for `scenario`, once its run of [`child`] passed, with neither backtrace
/// variable set.
fn printed(scenario: &str) -> String {
    printed_with(scenario, &[])
}

/// [`printed`] with `vars` set in the child's environment.
fn printed_with(scenario: &str, vars: &[(&str, &str)]) -> String {
    let exe = std::env::current_exe().expect("the test binary's path");
    let mut command = Command::new(exe);
    command.args(["--exact", "child", "--nocapture", "--test-threads=1"]).env(SCENARIO, scenario);
    command.env_remove("RUST_BACKTRACE").env_remove("RUST_LIB_BACKTRACE").envs(vars.iter().copied());
    let out = command.output().expect("the child runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success() && stdout.contains("1 passed"), "{scenario}: the child did not pass\nstdout: {stdout}\nstderr: {stderr}");
    stderr
}

/// Without the hook the default hook prints the raw message: the secret and the password reach
/// stderr however ulo redacts the panic afterwards.
#[test]
fn the_default_hook_prints_a_registered_secret_raw() {
    let stderr = printed("default");
    assert!(stderr.contains(SECRET) && stderr.contains(PASSWORD), "{stderr}");
}

/// With the hook the secret an app registered and the url's userinfo are replaced, the rest of
/// the message and the panic's location printed.
#[test]
fn the_redacting_hook_prints_a_registered_secret_redacted() {
    let stderr = printed("hook");
    assert!(!stderr.contains(SECRET) && !stderr.contains(PASSWORD), "{stderr}");
    assert!(stderr.contains("connecting with [redacted] to postgres://[redacted]@db/main"), "{stderr}");
    assert!(stderr.contains(&format!("panicked at {}:", file!())), "{stderr}");
}

/// The secrets are recorded when the app wires, whether the hook was installed before or after,
/// and stay recorded once the app has dropped.
#[test]
fn the_hook_redacts_an_app_s_secrets_installed_after_the_wire_and_after_the_app_dropped() {
    for scenario in ["hook_after_wire", "dropped"] {
        let stderr = printed(scenario);
        assert!(!stderr.contains(SECRET) && !stderr.contains(PASSWORD), "{scenario}: {stderr}");
        assert!(stderr.contains("connecting with [redacted]"), "{scenario}: {stderr}");
    }
}

/// The backtrace follows `RUST_BACKTRACE` as the default hook's does: `RUST_LIB_BACKTRACE=1`
/// alone prints none and the note, and `RUST_BACKTRACE=1` prints one under `RUST_LIB_BACKTRACE=0`.
#[test]
fn the_hook_s_backtrace_follows_rust_backtrace_alone() {
    let library_only = printed_with("hook", &[("RUST_LIB_BACKTRACE", "1")]);
    assert!(!library_only.contains("stack backtrace:"), "{library_only}");
    assert!(library_only.contains("note: run with `RUST_BACKTRACE=1`"), "{library_only}");
    let panic_only = printed_with("hook", &[("RUST_BACKTRACE", "1"), ("RUST_LIB_BACKTRACE", "0")]);
    assert!(panic_only.contains("stack backtrace:") && !panic_only.contains("note: run with"), "{panic_only}");
}

/// Before any app has wired the hook knows no secret, and only a url's userinfo is stripped.
#[test]
fn before_any_app_wires_the_hook_strips_userinfo_alone() {
    let stderr = printed("unwired");
    assert!(stderr.contains(SECRET), "{stderr}");
    assert!(!stderr.contains(PASSWORD), "{stderr}");
}
