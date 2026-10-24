//! Keys no section read (integrations DESIGN §2.4): `connect` warns about each, and
//! `.deny_unread()` refuses instead, naming every one with its path and the origin that supplied
//! it. `wire()` cannot do either, since a module loaded later may still read a key, so these run
//! through `connect`.

// `StartupError` is large by design; `connect` returns it as the app does.
#![allow(clippy::result_large_err)]

mod common;

use std::time::Duration;

use serde::Deserialize;
use ulo::{App, ConnectError, FailureReason, Module, ModuleDef, ModuleIdentity, StartupError};
use ulo_config::{Config, ConfigError, ConfigModule, Env, File, Origin, Values};

use common::{Dir, pairs, sorted};

const PATIENCE: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
struct Small {
    used: u16,
}

struct Root {
    config: Config,
    deny: bool,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let module = ConfigModule::<Small>::new(&self.config);
        m.import(if self.deny { module.deny_unread() } else { module });
    }
}

fn config(dir: &Dir) -> (Config, std::path::PathBuf) {
    let file = dir.write("app.toml", "used = 1\nextra = 3\n[nested]\nkey = 4\n");
    let config = Config::builder()
        .source(File::toml(&file))
        .source(Env::from_map([("APP_STRAY", "1"), ("APP_NESTED__OTHER", "2"), ("UNRELATED", "x")]).prefix("APP_"))
        .source(Values::new().set("used", 2))
        .load();
    (config, file)
}

async fn connect(config: Config, deny: bool) -> Result<ulo::App<ulo::Connected>, StartupError> {
    let wired = App::builder(Root { config, deny }).runtime(ulo_runtime_tokio::Tokio::current()).wire().unwrap_or_else(|error| panic!("the app wires: {error}"));
    tokio::time::timeout(PATIENCE, wired.connect()).await.unwrap_or_else(|_| panic!("`connect` did not finish within {PATIENCE:?}"))
}

/// `.deny_unread()` refuses at `connect`: the hook's error names each leaf nothing read, by its
/// path and its origin, a file's with its line and column, and leaves out the one a section read
/// whichever layer supplied it.
#[tokio::test(flavor = "current_thread")]
async fn deny_unread_refuses_at_connect_naming_every_unread_key_and_its_origin() {
    let dir = Dir::new();
    let (config, file) = config(&dir);
    let Err(StartupError::Connect(ConnectError::Hook { reason: FailureReason::Errored(error), .. })) = connect(config, true).await else {
        panic!("an app holding keys nothing reads connected under `.deny_unread()`");
    };
    let config_error = error.downcast_ref::<ConfigError>().expect("the hook's own error stays reachable");
    let path = file.to_string_lossy().into_owned();
    let expected = sorted(vec![
        ("extra".to_owned(), Some(Origin::File { path: path.as_str().into(), line: 2, column: 9 })),
        ("nested.key".to_owned(), Some(Origin::File { path: path.as_str().into(), line: 4, column: 7 })),
        ("stray".to_owned(), Some(Origin::Variable { name: "APP_STRAY".into() })),
        ("nested.other".to_owned(), Some(Origin::Variable { name: "APP_NESTED__OTHER".into() })),
    ]);
    assert_eq!(sorted(pairs(config_error.problems().iter())), expected, "`used` was read and `UNRELATED` has no prefix, so neither is listed");
}

/// Without `.deny_unread()` the same configuration connects: the keys are logged and nothing is
/// refused.
#[tokio::test(flavor = "current_thread")]
async fn unread_keys_are_only_a_warning_by_default() {
    let dir = Dir::new();
    let (config, _) = config(&dir);
    let connected = connect(config, false).await.unwrap_or_else(|error| panic!("unread keys refused a default app: {error}"));
    tokio::time::timeout(PATIENCE, connected.close(ulo::Signal::new("test"))).await.expect("the close finished").expect("the app closes");
}

/// With every key read, `.deny_unread()` has nothing to refuse.
#[tokio::test(flavor = "current_thread")]
async fn deny_unread_accepts_a_configuration_every_key_of_which_is_read() {
    let config = Config::builder().source(Values::new().set("used", 2)).load();
    let connected = connect(config, true).await.unwrap_or_else(|error| panic!("a configuration with no stray key was refused: {error}"));
    tokio::time::timeout(PATIENCE, connected.close(ulo::Signal::new("test"))).await.expect("the close finished").expect("the app closes");
}
