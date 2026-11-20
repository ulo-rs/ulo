//! A datagram socket inherited through the socket-activation protocol beside a listener, through
//! the server's `prepare` and `bind`: a server whose endpoint names the datagram socket fails
//! startup naming its index, and one naming the listener serves. The inheriting process is the test
//! binary run again as the ignored test `child_binds_an_inherited_endpoint`, a datagram socket at
//! descriptor 3 and a listener at 4, `LISTEN_PID` set to its own process ID by the shell it is
//! started through.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use command_fds::{CommandFdExt, FdMapping};
use ulo::app::Connected;
use ulo::{App, Signal};
use ulo_http_conformance::{Harness, OnTokio};
use ulo_net::Endpoint;

/// The name the child's endpoint gives; unset, the child test does nothing.
const CHILD: &str = "ULO_DATAGRAM_BESIDE_CHILD";

/// The child, its endpoint `Endpoint::inherited(name)`, and the port of the listener at descriptor
/// 4. The datagram socket is named `datagram`, the listener `web`.
fn spawn_child(name: &str) -> (Child, u16) {
    let datagram = UdpSocket::bind("127.0.0.1:0").expect("a datagram socket");
    let listener = TcpListener::bind("127.0.0.1:0").expect("a listener");
    let port = listener.local_addr().expect("its address").port();
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("LISTEN_PID=$$ exec \"$0\" \"$@\"")
        .arg(std::env::current_exe().expect("the test binary's path"))
        .args(["--exact", "child_binds_an_inherited_endpoint", "--ignored", "--nocapture", "--test-threads=1"])
        .env(CHILD, name)
        .env("LISTEN_FDS", "2")
        .env("LISTEN_FDNAMES", "datagram:web")
        .env_remove("LISTEN_PID")
        .env_remove("ULO_DEV")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    command
        .fd_mappings(vec![
            FdMapping { parent_fd: datagram.into(), child_fd: 3 },
            FdMapping { parent_fd: listener.into(), child_fd: 4 },
        ])
        .expect("two distinct targets");
    (command.spawn().expect("the child starts"), port)
}

/// The first line the child reports on, `ready` or `failed: <the error>`. The rest of its output is
/// read on a thread of its own, so the child's harness never writes to a closed pipe.
fn report(child: &mut Child) -> String {
    let stdout = child.stdout.take().expect("the child's stdout");
    let mut lines = BufReader::new(stdout).lines().map_while(Result::ok);
    // libtest writes the test's name before the first line the test prints, on the same line.
    let report = lines.find_map(|line| line.split_once("ulo-beside: ").map(|(_, report)| report.to_owned()));
    std::thread::spawn(move || lines.for_each(drop));
    report.expect("the child reports")
}

/// Closes the child's stdin, which stops its app, and waits for it.
fn stop(mut child: Child) {
    drop(child.stdin.take());
    child.wait().expect("the child exits");
}

#[test]
#[cfg_attr(miri, ignore = "Miri refuses `socket` and `Command` under isolation")]
fn a_server_naming_the_datagram_socket_fails_startup_naming_its_index() {
    let (mut child, _) = spawn_child("datagram");
    let report = report(&mut child);
    stop(child);
    assert!(report.starts_with("failed: "), "{report}");
    assert!(report.contains("inherited descriptor #0 is not a listening TCP socket"), "{report}");
}

#[test]
#[cfg_attr(miri, ignore = "Miri refuses `socket` and `Command` under isolation")]
fn a_server_naming_the_listener_beside_it_serves() {
    let (mut child, port) = spawn_child("web");
    let report = report(&mut child);
    assert_eq!(report, "ready");
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("the inherited listener accepts");
    stream.set_read_timeout(Some(Duration::from_secs(10))).expect("a read timeout");
    stream.write_all(b"GET /hit HTTP/1.1\r\nHost: beside\r\nConnection: close\r\n\r\n").expect("the request is written");
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).expect("the answer is read to its end");
    stop(child);
    let answer = String::from_utf8_lossy(&answer);
    assert!(answer.starts_with("HTTP/1.1 200") && answer.ends_with("hit"), "{answer:?}");
}

/// The child: the suite's app on tokio, bound to the inherited endpoint `ULO_DATAGRAM_BESIDE_CHILD`
/// names, serving until its stdin closes.
#[test]
#[ignore = "the child process of the tests above; does nothing unless they start it"]
fn child_binds_an_inherited_endpoint() {
    let Ok(name) = std::env::var(CHILD) else { return };
    let (closed, stdin_closed) = futures::channel::oneshot::channel::<()>();
    std::thread::spawn(move || {
        let _ = std::io::stdin().read_to_end(&mut Vec::new());
        let _ = closed.send(());
    });
    let serve = |app: App<Connected>| async move {
        match app.bind(ulo_http_hyper::Server::new(Endpoint::inherited(name))).listen().await {
            Ok(app) => {
                println!("ulo-beside: ready");
                let _ = app
                    .serve(async move {
                        let _ = stdin_closed.await;
                        Signal::new("stdin closed")
                    })
                    .await;
            }
            Err(error) => println!("ulo-beside: failed: {}", error.to_string().replace('\n', " ")),
        }
    };
    OnTokio::block_on(async { serve(ulo_http_conformance::app(OnTokio::runtime()).await).await });
}
