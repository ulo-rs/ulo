//! A provider cycle that crosses a module boundary is refused at startup, and
//! the message names every provider on the cycle.
//!
//! Rust makes the cycle a hang or a stack overflow rather than a type error, so
//! the diagnostic is the whole feature: a refusal naming only the entry point
//! leaves the author to find the other half by hand.
#![allow(dead_code)]

use ulo::*;

// Global modules keep the import graph acyclic — both get ordered and reach the injector's
// Phase-1 stall — while the providers still form a `ServiceA` <-> `ServiceB` cycle. `#[new]`
// constructor injection consumes each dependency without storing it, so neither struct embeds
// the other and both stay finitely sized.
#[injectable]
pub struct ServiceA {
    name: String,
}

impl ServiceA {
    #[new]
    pub fn new(_b: ServiceB) -> Self {
        Self { name: "a".into() }
    }
}

#[injectable]
pub struct ServiceB {
    name: String,
}

impl ServiceB {
    #[new]
    pub fn new(_a: ServiceA) -> Self {
        Self { name: "b".into() }
    }
}

#[module(providers: [ServiceA], exports: [ServiceA], global: true)]
impl ModuleA {}

#[module(providers: [ServiceB], exports: [ServiceB], global: true)]
impl ModuleB {}

#[module(imports: [ModuleA, ModuleB])]
impl AppModule {}

/// A cross-module provider cycle must fail with a diagnostic that names the exact providers in
/// the cycle, not just the modules involved.
#[tokio::test]
async fn cross_module_provider_cycle_names_the_exact_providers() {
    let message = UloFactory::create_application_context(AppModule)
        .await
        .err()
        .expect("a cross-module provider cycle must fail initialization")
        .to_string();

    assert!(
        message.contains("Circular dependency detected between providers"),
        "expected the sharpened cycle diagnostic, got:\n{message}"
    );
    assert!(
        message.contains("ServiceA") && message.contains("ServiceB"),
        "diagnostic should name both providers in the cycle, got:\n{message}"
    );
    assert!(
        message.contains("Break the cycle"),
        "diagnostic should include the remediation guidance, got:\n{message}"
    );
}
