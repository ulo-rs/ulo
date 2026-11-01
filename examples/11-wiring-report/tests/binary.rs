use ex_support::Bin;

#[test]
fn the_binary_prints_the_report_on_stderr_and_exits_1() {
    let exit = Bin::spawn(env!("CARGO_BIN_EXE_ex-11-wiring-report"), &[]).finish();
    assert_eq!(exit.status.code(), Some(1), "{exit:?}");
    assert!(exit.stderr.starts_with("error: wiring failed with 5 errors"), "{}", exit.stderr);
    assert_eq!(exit.stdout, Vec::<String>::new());
}
