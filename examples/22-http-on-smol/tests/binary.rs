use std::io::{Read, Write};
use std::net::TcpStream;

use ex_support::Bin;

#[test]
fn the_binary_serves_on_the_port_it_is_given_and_exits_clean_on_sigterm() {
    let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-22-http-on-smol"), &[]);
    let mut stream = TcpStream::connect(bin.addr()).expect("the printed address accepts connections");
    stream.write_all(b"GET / HTTP/1.1\r\nhost: example\r\nconnection: close\r\n\r\n").unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 200") && reply.ends_with("Hello, ulo!"), "{reply}");
    let exit = bin.terminate();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stderr, "");
}
