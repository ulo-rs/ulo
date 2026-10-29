use std::collections::HashMap;

use ex_05_scopes::{Ids, PerCall, Shared, builder};
use ex_support::{get, start};
use ulo::{App, ScopeKind, StartupError, WiringError, Dep, injectable, module};

/// `"singleton=1/1 execution=3/3 .."` as a map from scope to its two ids.
fn read(body: &str) -> HashMap<&str, (u64, u64)> {
    body.split(' ')
        .map(|part| {
            let (scope, ids) = part.split_once('=').unwrap();
            let (a, b) = ids.split_once('/').unwrap();
            (scope, (a.parse().unwrap(), b.parse().unwrap()))
        })
        .collect()
}

#[tokio::test]
async fn counts_per_request_and_across_requests_match_each_scope() {
    let server = start(builder()).await;
    let one = get(server.addr, "/").await.body;
    let two = get(server.addr, "/").await.body;
    let (one, two) = (read(&one), read(&two));

    for scope in ["singleton", "auto"] {
        let (a, b) = one[scope];
        assert_eq!(a, b, "{scope}: both reads in a request see one instance");
        assert_eq!(one[scope], two[scope], "{scope}: the next request sees it too");
    }
    let (a, b) = one["execution"];
    assert_eq!(a, b, "execution: shared inside a request");
    assert_ne!(one["execution"], two["execution"], "execution: a new instance per request");
    let (a, b) = one["transient"];
    assert_ne!(a, b, "transient: fresh at every injection point");
    assert_ne!(one["transient"], two["transient"]);
    assert_ne!(one["singleton"].0, one["auto"].0, "the two singletons are two instances");

    // A singleton can be read from outside a request, and is the one the requests read.
    let shared: Dep<Shared> = server.handle.get().await.unwrap();
    assert_eq!(shared.0, one["singleton"].0);
    server.stop().await.unwrap();
}

#[injectable(singleton)]
struct Pinned {
    _per_call: Dep<PerCall>,
}

#[module(providers = [Ids, PerCall, Pinned])]
struct PinnedModule;

#[test]
fn a_singleton_reading_a_per_execution_binding_is_refused_with_the_path() {
    let Err(StartupError::Wiring(errors)) = App::builder(PinnedModule).wire() else {
        panic!("wiring should fail");
    };
    let [WiringError::ScopeViolation { binding, declared, by_closure, module, path }] =
        errors.iter().collect::<Vec<_>>()[..]
    else {
        panic!("expected one scope violation, got {errors}");
    };
    assert_eq!(binding.to_string(), "Pinned");
    assert_eq!(*declared, ScopeKind::Singleton);
    assert!(!by_closure);
    assert_eq!(module.to_string(), "PinnedModule");
    assert_eq!(path.join(" -> "), "Pinned -> PerCall (execution)");
}
