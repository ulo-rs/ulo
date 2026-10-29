use ex_support::Bin;

#[test]
fn the_binary_prints_the_journal_from_the_first_readiness_attempt_to_the_last_shutdown_hook() {
    let exit = Bin::spawn(env!("CARGO_BIN_EXE_ex-09-lifecycle"), &[]).finish();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stdout.len(), 11, "{:?}", exit.stdout);
    assert_eq!(exit.stdout.first().map(String::as_str), Some("Link ready: Err(\"link refused attempt 1\")"));
    assert_eq!(exit.stdout.last().map(String::as_str), Some("Cache shutdown (done)"));
    assert_eq!(exit.stderr, "");
}
