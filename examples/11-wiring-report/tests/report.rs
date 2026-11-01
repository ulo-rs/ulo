use ex_11_wiring_report::builder;
use ulo::{ScopeKind, StartupError, WiringError};

fn lib_rs(location: &std::panic::Location<'_>) -> bool {
    location.file().ends_with("11-wiring-report/src/lib.rs")
}

#[test]
fn one_wire_call_reports_every_problem_with_its_module_key_and_location() {
    let Err(StartupError::Wiring(errors)) = builder().wire() else {
        panic!("the broken app should fail wiring");
    };
    assert_eq!(errors.len(), 5);
    let all: Vec<&WiringError> = errors.iter().collect();

    let [duplicate, missing, ambiguous, cycle, scope] = all[..] else { unreachable!() };

    let WiringError::DuplicateBinding { key, module, first, second } = duplicate else { panic!("{duplicate:?}") };
    assert_eq!((key.to_string(), module.to_string()), ("u32".into(), "TwiceModule".into()));
    assert!(lib_rs(first) && lib_rs(second) && second.line() == first.line() + 1);

    let WiringError::Missing { key, consumer, module, near } = missing else { panic!("{missing:?}") };
    assert_eq!((key.to_string(), consumer.as_str(), module.to_string()), ("dyn Mailer".into(), "Notifier (field `_mailer`)", "BrokenModule".into()));
    assert!(near.is_none());

    let WiringError::Ambiguous { key, consumer, module, sources } = ambiguous else { panic!("{ambiguous:?}") };
    assert_eq!((key.to_string(), consumer.as_str(), module.to_string()), ("dyn Repo".into(), "Reports (field `_repo`)", "BrokenModule".into()));
    let exporters: Vec<String> = sources.iter().map(|(module, _)| module.to_string()).collect();
    assert_eq!(exporters, ["SqlModule", "MemoryModule"]);
    assert!(sources.iter().all(|(_, at)| lib_rs(at)));

    let WiringError::Cycle { path } = cycle else { panic!("{cycle:?}") };
    let steps: Vec<(String, String)> = path.iter().map(|(key, module)| (key.to_string(), module.to_string())).collect();
    assert_eq!(steps, [("Chicken", "BrokenModule"), ("Egg", "BrokenModule"), ("Chicken", "BrokenModule")].map(|(k, m)| (k.into(), m.into())));

    let WiringError::ScopeViolation { binding, declared, module, path, .. } = scope else { panic!("{scope:?}") };
    assert_eq!((binding.to_string(), module.to_string()), ("Directory".into(), "BrokenModule".into()));
    assert_eq!(*declared, ScopeKind::Singleton);
    assert_eq!(path.join(" -> "), "Directory -> Session (execution)");
}

#[test]
fn the_report_prints_one_block_per_problem_for_a_human() {
    let Err(StartupError::Wiring(errors)) = builder().wire() else { panic!("the broken app should fail wiring") };
    let report = errors.to_string();
    for expected in [
        "wiring failed with 5 errors",
        "duplicate binding for `u32` in TwiceModule",
        "missing dependency `dyn Mailer`",
        "needed by Notifier (field `_mailer`) in BrokenModule",
        "ambiguous dependency `dyn Repo` in BrokenModule",
        "exported by SqlModule",
        "exported by MemoryModule",
        "dependency cycle: Chicken → Egg → Chicken",
        "scope violation: singleton `Directory` in BrokenModule depends on per-execution data",
    ] {
        assert!(report.contains(expected), "the report lacks {expected:?}:\n{report}");
    }
}
