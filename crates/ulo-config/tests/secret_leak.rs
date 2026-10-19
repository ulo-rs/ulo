//! Premise check R3: no secret value appears in a rendered problem, under `flatten` and
//! `untagged` included.

use serde::{Deserialize, Deserializer, de::Error as _};
use ulo::Secret;
use ulo_config::{Config, Section, Values};

const SENTINEL: &str = "hunter2-sentinel";

/// A leaf type whose own `Deserialize` quotes its input in `Error::custom`, as `Url` or `IpAddr`
/// style parsers do.
#[derive(Debug)]
struct Quoting(#[allow(dead_code)] String);

impl<'de> Deserialize<'de> for Quoting {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Err(D::Error::custom(format!("cannot parse `{text}`")))
    }
}

#[derive(Debug, Deserialize)]
struct Inner<S> {
    #[allow(dead_code)]
    password: Secret<S>,
}

#[derive(Debug, Deserialize)]
struct Flat<S> {
    #[allow(dead_code)]
    name: String,
    #[serde(flatten)]
    #[allow(dead_code)]
    inner: Inner<S>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Either<S> {
    A {
        #[allow(dead_code)]
        password: Secret<S>,
        #[allow(dead_code)]
        extra: u32,
    },
    B(#[allow(dead_code)] Inner<S>),
}

#[derive(Debug, Deserialize)]
struct Direct<S> {
    #[allow(dead_code)]
    password: Secret<S>,
}

fn config() -> Config {
    Config::builder().source(Values::new().set("name", "n").set("password", SENTINEL)).build_for_test()
}

trait Build {
    fn build_for_test(self) -> Config;
}
impl Build for ulo_config::ConfigBuilder {
    fn build_for_test(self) -> Config {
        self.load()
    }
}

/// Every rendering the crate offers of a failed section, joined.
fn render<T: for<'de> Deserialize<'de> + std::fmt::Debug>() -> String {
    let section = Section::<T>::read(&config(), "");
    let error = section.value().expect_err("the read must fail");
    let mut out = format!("{error}\n{error:?}\n{section:?}\n");
    for problem in error.problems().iter() {
        out += &format!("{problem:?} {} {} {:?}\n", problem.path(), problem.message(), problem.origin());
    }
    out += &format!("{:?}\n", error.problems());
    out
}

fn assert_clean(label: &str, rendered: &str) {
    assert!(!rendered.contains(SENTINEL), "{label} leaked the secret:\n{rendered}");
}

#[test]
fn control_direct_secret_with_wrong_type_and_custom_error_leaks_nothing() {
    assert_clean("direct wrong type", &render::<Direct<u32>>());
    assert_clean("direct custom", &render::<Direct<Quoting>>());
}

#[test]
fn wrong_type_under_flatten_leaks_nothing() {
    let out = render::<Flat<u32>>();
    assert!(out.contains("expected u32"), "the problem must still say what was wrong:\n{out}");
    assert_clean("flatten wrong type", &out);
}

#[test]
fn custom_error_under_flatten_leaks_nothing() {
    assert_clean("flatten custom", &render::<Flat<Quoting>>());
}

#[test]
fn wrong_type_in_untagged_variant_leaks_nothing() {
    assert_clean("untagged wrong type", &render::<Either<u32>>());
}

#[test]
fn custom_error_in_untagged_variant_leaks_nothing() {
    assert_clean("untagged custom", &render::<Either<Quoting>>());
}
