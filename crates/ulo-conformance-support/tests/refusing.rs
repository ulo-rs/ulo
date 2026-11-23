//! `relay::refusing_addr` refuses every connect, and no IPv4 socket bound after it is given its
//! port.
//! A listener held open, another way to keep a port, completes a connect it never accepts.

use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use socket2::{Domain, Socket, Type};
use ulo_conformance_support::relay::refusing_addr;

/// How long a connect may take; a loopback connect answers in milliseconds.
const PATIENCE: Duration = Duration::from_secs(5);

/// How many sockets of each kind bound to port 0 the reservation is tested against. A listener std
/// binds is marked `SO_REUSEADDR`, which Linux hands a random odd port of the lower half of its
/// default ephemeral range, 7 058 ports; a socket not marked is handed a random odd port of the
/// whole range, 14 116, which holds the refusing port wherever it lies. This many reach all but a
/// fraction of a percent of each, and a port bound and released before them is handed out again.
#[cfg(target_os = "linux")]
const EPHEMERAL_BINDS: usize = 100_000;

/// A sample elsewhere. macOS keeps a closed socket's control block until a periodic sweep: the
/// Linux count leaves over 100 000 alive at once, which beside other test processes exhausts the
/// kernel's supply, and every socket call on the host then fails with `ENOBUFS`. A port-0
/// bind passes over a port held in the kernel's table, which
/// `a_socket_not_marked_cannot_bind_the_refusing_address` pins; these draws of macOS's 16 384
/// ports check it without covering the range.
#[cfg(not(target_os = "linux"))]
const EPHEMERAL_BINDS: usize = 1_000;

#[test]
fn every_connect_to_the_refusing_address_is_refused() {
    let addr = refusing_addr();
    for attempt in 1..=3 {
        let error = TcpStream::connect_timeout(&addr, PATIENCE).map(drop).expect_err("a connect to the refusing address succeeded");
        assert_eq!(error.kind(), ErrorKind::ConnectionRefused, "connect {attempt} to {addr} failed otherwise than as refused: {error}");
    }
}

#[test]
fn a_listener_held_open_completes_a_connect_it_never_accepts() {
    let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a loopback listener");
    let addr = held.local_addr().expect("its address");
    TcpStream::connect_timeout(&addr, PATIENCE).unwrap_or_else(|error| panic!("a connect to a held listener at {addr} failed: {error}"));
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "BSD's `SO_REUSEADDR`, which std sets, lets a listener bind a port a connected socket holds")]
fn a_listener_cannot_bind_the_refusing_address() {
    let addr = refusing_addr();
    let error = TcpListener::bind(addr).map(drop).expect_err("a listener bound the refusing address");
    assert_eq!(error.kind(), ErrorKind::AddrInUse, "binding {addr} failed otherwise than as in use: {error}");
}

#[test]
fn a_socket_not_marked_cannot_bind_the_refusing_address() {
    let addr = refusing_addr();
    let unmarked = Socket::new(Domain::IPV4, Type::STREAM, None).expect("a socket");
    let error = unmarked.bind(&addr.into()).expect_err("a socket not marked `SO_REUSEADDR` bound the refusing address");
    assert_eq!(error.kind(), ErrorKind::AddrInUse, "binding {addr} failed otherwise than as in use: {error}");
}

#[test]
fn no_ipv4_socket_bound_to_port_zero_is_given_the_refusing_port() {
    let port = refusing_addr().port();
    for bind in 1..=EPHEMERAL_BINDS {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a loopback listener");
        let given = listener.local_addr().expect("its address").port();
        assert_ne!(given, port, "listener {bind} bound to port 0 was given the refusing port");
        let unmarked = Socket::new(Domain::IPV4, Type::STREAM, None).expect("a socket");
        unmarked.bind(&SocketAddr::from((Ipv4Addr::LOCALHOST, 0)).into()).expect("a loopback port");
        let given = unmarked.local_addr().ok().and_then(|addr| addr.as_socket()).expect("its address").port();
        assert_ne!(given, port, "socket {bind} bound to port 0 without `SO_REUSEADDR` was given the refusing port");
    }
}
