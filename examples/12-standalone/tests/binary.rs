use ex_support::Bin;

#[test]
fn the_binary_prints_the_report_and_the_signal_it_closed_on() {
    let exit = Bin::spawn(env!("CARGO_BIN_EXE_ex-12-standalone"), &[]).finish();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stdout, ["acme: 4 rows", "closed on done"]);
    assert_eq!(exit.stderr, "");
}
