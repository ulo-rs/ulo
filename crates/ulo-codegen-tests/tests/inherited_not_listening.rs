//! A server whose endpoint names an inherited descriptor that is not a listening TCP socket fails
//! `listen` naming the endpoint and the descriptor, on each of the four servers that adopt an
//! inherited socket: HTTP, WebSocket, RPC over TCP and gRPC. The inheriting process is the test
//! binary run again as the ignored test `child_listens_on_a_datagram_socket`, a datagram socket at
//! descriptor 3 named `datagram`, `LISTEN_PID` set to its own process ID by the shell it is started
//! through.
#![cfg(unix)]

use std::net::UdpSocket;
use std::process::{Command, Stdio};

use command_fds::{CommandFdExt, FdMapping};
use ulo::{App, Module, ModuleDef, ModuleIdentity};
use ulo_net::Endpoint;

/// Set in the child; unset, the child test does nothing.
const CHILD: &str = "ULO_NOT_LISTENING_CHILD";

/// What the child prints before the startup error, so the parent finds it among libtest's lines.
const MARK: &str = "ulo-not-listening: ";

#[test]
#[cfg_attr(miri, ignore = "Miri refuses `socket` and `Command` under isolation")]
fn every_server_naming_a_datagram_socket_fails_listen_naming_the_endpoint_and_the_descriptor() {
    let datagram = UdpSocket::bind("127.0.0.1:0").expect("a datagram socket");
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("LISTEN_PID=$$ exec \"$0\" \"$@\"")
        .arg(std::env::current_exe().expect("the test binary's path"))
        .args(["--exact", "child_listens_on_a_datagram_socket", "--ignored", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("LISTEN_FDS", "1")
        .env("LISTEN_FDNAMES", "datagram")
        .env_remove("LISTEN_PID")
        .env_remove("ULO_DEV")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.fd_mappings(vec![FdMapping { parent_fd: datagram.into(), child_fd: 3 }]).expect("one target");
    let output = command.output().expect("the child runs");
    let text = String::from_utf8_lossy(&output.stdout);
    let report = text
        .lines()
        .find_map(|line| line.split_once(MARK).map(|(_, report)| report.to_owned()))
        .unwrap_or_else(|| panic!("the child reported nothing:\n{text}{}", String::from_utf8_lossy(&output.stderr)));
    let reason = "endpoint `datagram`: inherited descriptor #0 is not a listening TCP socket";
    for transport in ["Http", "Ws", "Rpc", "Grpc"] {
        let entry = report
            .split('×')
            .find(|entry| entry.trim_start().starts_with(&format!("transport `{transport}`:")))
            .unwrap_or_else(|| panic!("no entry for `{transport}`: {report}"));
        assert!(entry.contains(reason), "`{transport}` does not name the endpoint: {entry}");
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, _m: &mut ModuleDef<'_>) {}
}

/// The child: an app binding the four servers to the inherited endpoint `datagram`, printing what
/// `listen` answers on one line.
#[test]
#[ignore = "the child process of the test above; does nothing unless it starts it"]
fn child_listens_on_a_datagram_socket() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    runtime.block_on(async {
        let app = App::builder(Root).runtime(ulo_runtime_tokio::Tokio::current()).wire().expect("the app wires");
        let app = app.connect().await.expect("the app connects");
        let datagram = || Endpoint::inherited("datagram");
        let answer = app
            .bind(ulo_http_hyper::Server::new(datagram()))
            .bind(ulo_ws_hyper::Server::new(datagram()))
            .bind(ulo_rpc::Server::new(ulo_rpc_tcp::Tcp::new(datagram())))
            .bind(ulo_grpc::Server::new(datagram()))
            .listen()
            .await;
        match answer {
            Ok(_) => println!("{MARK}listened"),
            Err(error) => println!("{MARK}{}", error.to_string().replace('\n', " ")),
        }
    });
}
