//! The compiled binary under a supervisor: a parent opens a listening socket and starts the binary
//! with it as descriptor 3 and `LISTEN_FDS=1`, which is what systemd and `ulo dev --listen` do.
//! Unix only, since the protocol is.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::os::fd::OwnedFd;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use ex_support::{GUARD, get};

/// Runs the binary with `socket` as descriptor 3 and `LISTEN_FDS=1`. `LISTEN_PID` is the child's
/// own process ID, which the shell knows as `$$` and keeps across the `exec`, unless `pid` names
/// another. The socket crosses on stdin and is duplicated to descriptor 3 by the shell, so the
/// test needs no `unsafe`.
struct Supervised {
    child: Child,
    lines: Receiver<String>,
    stderr: Option<thread::JoinHandle<String>>,
}

impl Supervised {
    fn start(socket: impl Into<OwnedFd>, pid: Option<&str>) -> Supervised {
        let script = r#"LISTEN_PID=${1:-$$} LISTEN_FDS=1 exec "$0" 3<&0 </dev/null"#;
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script, env!("CARGO_BIN_EXE_ex-40-socket-activation")]);
        command.args(pid);
        let mut child = command
            .stdin(Stdio::from(socket.into()))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the shell starts");
        let stdout = child.stdout.take().expect("stdout is piped");
        let (send, lines) = channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let mut stderr = child.stderr.take().expect("stderr is piped");
        let stderr = Some(thread::spawn(move || {
            let mut text = String::new();
            stderr.read_to_string(&mut text).ok();
            text
        }));
        Supervised { child, lines, stderr }
    }

    fn listening_line(&self) -> String {
        self.lines.recv_timeout(GUARD).expect("the binary prints its address")
    }

    fn signal(&self, name: &str) {
        let status = Command::new("/bin/kill").args([name, &self.child.id().to_string()]).status().expect("kill runs");
        assert!(status.success(), "{status}");
    }

    /// Waits for the exit; returns whether it succeeded and what the binary wrote to stderr.
    fn finish(mut self) -> (bool, String) {
        let started = Instant::now();
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("the child can be polled") {
                break status;
            }
            assert!(started.elapsed() < GUARD, "the binary was still running {GUARD:?} later");
            thread::sleep(Duration::from_millis(5));
        };
        (status.success(), self.stderr.take().expect("read once").join().expect("reader joins"))
    }
}

impl Drop for Supervised {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            self.child.kill().ok();
            self.child.wait().ok();
        }
    }
}

fn read_reply(mut stream: TcpStream) -> String {
    stream.set_read_timeout(Some(GUARD)).unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).expect("the reply is read to the end");
    reply
}

#[tokio::test]
async fn a_connection_made_before_the_app_starts_is_served_once_it_is_up() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    // The supervisor holds the socket, so this connection completes and its request waits in the
    // backlog while no application exists.
    let mut early = TcpStream::connect(addr).unwrap();
    early.write_all(b"GET / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n").unwrap();

    let app = Supervised::start(listener, None);
    let reply = read_reply(early);
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(reply.ends_with("Hello from a socket the app did not open"), "{reply}");

    assert_eq!(app.listening_line(), format!("listening on http://{addr}"));
    let late = get(addr, "/").await;
    assert_eq!((late.status, late.body.as_str()), (200, "Hello from a socket the app did not open"));

    app.signal("-TERM");
    let (clean, stderr) = app.finish();
    assert!(clean);
    assert_eq!(stderr, "");
}

#[test]
fn a_descriptor_that_is_not_a_listening_tcp_socket_fails_startup_naming_it() {
    let datagrams = UdpSocket::bind("127.0.0.1:0").unwrap();
    let app = Supervised::start(datagrams, None);
    let (clean, stderr) = app.finish();
    assert!(!clean);
    assert!(stderr.contains("inherited descriptor #0 is not a listening TCP socket"), "{stderr}");
}

#[test]
fn a_socket_meant_for_another_process_fails_startup_naming_both_pids() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let app = Supervised::start(listener, Some("1"));
    let (clean, stderr) = app.finish();
    assert!(!clean);
    assert!(stderr.contains("LISTEN_PID is 1, and this process is "), "{stderr}");
}
