//! The secrets of a configured `#[module]`'s fields, registered by type. Each module holds the
//! secret in one spelling of its field type and is imported by a root whose singleton fails with a
//! message carrying the secret; the `connect()` report then holds the secret only where the
//! field's registration was missed.

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};

use ulo::{App, Module, ModuleDef, ModuleIdentity, Secret, module};

const SECRET: &str = "module-field-secret";

type Pw = Secret<String>;

#[derive(Clone, PartialEq, Eq, Hash)]
#[module]
struct Plain {
    pw: Secret<String>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
#[module]
struct Qualified {
    pw: ulo::Secret<String>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
#[module]
struct Aliased {
    pw: Pw,
}

#[derive(Clone, PartialEq, Eq, Hash)]
#[module]
struct Optional {
    pw: Option<Secret<String>>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
#[module]
struct Listed {
    pws: Vec<Secret<String>>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
#[module]
struct Mapped {
    pws: BTreeMap<String, Secret<String>>,
}

// `HashMap` is not `Hash`, so the module hashes the keys it holds.
#[derive(Clone, PartialEq, Eq)]
#[module]
struct Hashed {
    pws: HashMap<String, Secret<String>>,
}

impl Hash for Hashed {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let mut keys: Vec<&String> = self.pws.keys().collect();
        keys.sort();
        keys.hash(state);
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
#[module]
struct Tupled(u16, Secret<String>);

#[derive(Clone, PartialEq, Eq, Hash)]
struct Credentials {
    pw: Secret<String>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
#[module]
struct Nested {
    credentials: Credentials,
}

/// Declared under the `cfg` the gated fields carry, so no build of this file has it.
#[cfg(any())]
#[derive(Clone, PartialEq, Eq, Hash)]
struct Absent;

#[derive(Clone, PartialEq, Eq, Hash)]
#[module]
struct Gated {
    #[cfg(any())]
    absent: Absent,
    #[cfg(not(any()))]
    pw: Secret<String>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
#[module]
struct GatedLast(Secret<String>, #[cfg(any())] Absent);

struct Failing;

/// Imports `M` and fails its one singleton with a message carrying [`SECRET`].
struct Root<M>(M);

impl<M: Module + Clone> Module for Root<M> {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(self.0.clone());
        m.try_singleton(|| async { Err::<Failing, _>(std::io::Error::other(format!("connect failed with {SECRET}"))) });
    }
}

async fn report(module: impl Module + Clone) -> String {
    let wired = App::builder(Root(module)).wire().expect("wires");
    match wired.connect().await {
        Err(error) => error.to_string(),
        Ok(_) => panic!("connect() succeeded; the root's singleton fails"),
    }
}

fn secret() -> Secret<String> {
    Secret::new(SECRET.to_owned())
}

async fn assert_redacted(module: impl Module + Clone) {
    let text = report(module).await;
    assert!(text.contains("connect failed with"), "the failure is reported: {text}");
    assert!(!text.contains(SECRET), "the field's secret is printed:\n{text}");
}

#[tokio::test]
async fn a_secret_field_is_registered() {
    assert_redacted(Plain { pw: secret() }).await;
}

#[tokio::test]
async fn a_secret_named_by_its_path_is_registered() {
    assert_redacted(Qualified { pw: secret() }).await;
}

#[tokio::test]
async fn a_secret_named_through_an_alias_is_registered() {
    assert_redacted(Aliased { pw: secret() }).await;
}

#[tokio::test]
async fn an_optional_secret_is_registered() {
    assert_redacted(Optional { pw: Some(secret()) }).await;
}

#[tokio::test]
async fn each_secret_of_a_vec_is_registered() {
    assert_redacted(Listed { pws: vec![Secret::new("another-secret".to_owned()), secret()] }).await;
}

#[tokio::test]
async fn each_secret_value_of_a_map_is_registered() {
    assert_redacted(Mapped { pws: BTreeMap::from([("db".to_owned(), secret())]) }).await;
}

#[tokio::test]
async fn each_secret_value_of_a_hash_map_is_registered() {
    assert_redacted(Hashed { pws: HashMap::from([("db".to_owned(), secret())]) }).await;
}

#[tokio::test]
async fn a_tuple_module_s_secret_field_is_registered() {
    assert_redacted(Tupled(5432, secret())).await;
}

/// A field behind a `cfg` that is off is not probed, and its type exists in no build; the secret
/// beside it, behind a `cfg` that is on, is registered. A tuple module's last field may be gated.
#[tokio::test]
async fn a_gated_field_is_probed_under_its_cfg() {
    assert_redacted(Gated { pw: secret() }).await;
    assert_redacted(GatedLast(secret())).await;
}

/// A `Secret` inside a struct the field holds is not reached; the docs send it to `m.secret` or to
/// configuration.
#[tokio::test]
async fn a_secret_inside_a_field_s_own_struct_is_not_registered() {
    let text = report(Nested { credentials: Credentials { pw: secret() } }).await;
    assert!(text.contains(SECRET), "the nested secret was registered, which the probe does not reach:\n{text}");
}
