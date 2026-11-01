use ex_support::Bin;

#[test]
fn the_binary_prints_what_the_macro_form_in_example_06_prints() {
    let exit = Bin::spawn(env!("CARGO_BIN_EXE_ex-14-without-macros"), &[]).finish();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stdout, ["invoice for ada registered with hashed:secret"]);
    assert_eq!(exit.stderr, "");
}
