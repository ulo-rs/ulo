//! Two `#[module]` structs importing each other: an import is a visibility fact, so the cycle
//! wires and connects, and a provider of each module reads the other's export through it
//! (DESIGN section 8.2).

use ulo::{App, Dep, injectable, module};

#[injectable]
struct ServiceC {
    d: Dep<ServiceD>,
}

#[injectable]
struct ServiceD;

/// Reads `ServiceC` through `ModuleD`'s import of `ModuleC`, the import closing the cycle.
#[injectable]
struct Audit {
    c: Dep<ServiceC>,
}

#[module(imports = [ModuleD], providers = [ServiceC], exports = [ServiceC])]
struct ModuleC;

#[module(imports = [ModuleC], providers = [ServiceD, Audit], exports = [ServiceD, Audit])]
struct ModuleD;

#[module(imports = [ModuleC, ModuleD])]
struct Root;

/// Planted break: the registration walk refusing every import that closes a cycle.
#[tokio::test]
async fn cross_module_dependency_through_an_import_cycle_resolves() {
    let app = App::builder(Root).wire().expect("an import cycle wires").connect().await.expect("connects");

    let c = app.get::<ServiceC>().await.expect("ServiceC reads ServiceD across the cycle");
    let audit = app.get::<Audit>().await.expect("Audit reads ServiceC across the cycle");
    assert!(
        std::sync::Arc::ptr_eq(audit.c.arc(), c.arc()),
        "both modules of the cycle hold the one ServiceC singleton"
    );
    let d = app.get::<ServiceD>().await.expect("the root reads ModuleD's export");
    assert!(std::sync::Arc::ptr_eq(c.d.arc(), d.arc()), "ServiceC holds ModuleD's ServiceD singleton");
}
