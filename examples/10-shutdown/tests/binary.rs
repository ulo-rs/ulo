use std::net::TcpStream;

use ex_support::Bin;

#[test]
fn the_binary_listens_on_the_port_it_is_given_and_reports_its_shutdown_on_sigterm() {
    let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-10-shutdown"), &[]);
    TcpStream::connect(bin.addr()).expect("the printed address accepts connections");
    let exit = bin.terminate();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stdout.last().map(String::as_str), Some("closed on SIGTERM: 0 abandoned"));
    assert_eq!(exit.stderr, "");
}
