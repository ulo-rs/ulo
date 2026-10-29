//! Runs an example's compiled binary the way a user's shell does, for the smoke test every example
//! carries: `main` is the one part of an example the library's tests do not reach.
//!
//! A server example's `main` reads `PORT` (default 3000) and prints one `listening on http://ADDR`
//! line per bound address; its smoke test starts it with [`Bin::server`], which hands it a free port
//! and checks the printed address is that port. Every wait is bounded by [`GUARD`] of real time, so
//! a binary that never prints or never exits fails the test instead of hanging it.

use std::io::{BufRead, BufReader, Read};
use std::net::SocketAddr;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The most real time any wait on the binary may take.
pub const GUARD: Duration = Duration::from_secs(10);

/// A running binary. Dropping it kills the process, so a failed assertion leaves nothing listening.
pub struct Bin {
    child: Child,
    lines: Receiver<String>,
    seen: Vec<String>,
    stderr: Option<JoinHandle<String>>,
    port: Option<u16>,
}

/// How the binary ended and everything it printed.
#[derive(Debug)]
pub struct Exit {
    pub status: ExitStatus,
    pub stdout: Vec<String>,
    pub stderr: String,
}

impl Bin {
    /// Starts the binary at `exe` (`env!("CARGO_BIN_EXE_<package name>")`) with `envs` added.
    pub fn spawn(exe: &str, envs: &[(&str, &str)]) -> Bin {
        let mut child = Command::new(exe)
            .envs(envs.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the example binary starts");
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
        Bin { child, lines, seen: Vec::new(), stderr, port: None }
    }

    /// Starts a server example with `PORT` set to a port that was free a moment ago, so [`addr`](Self::addr)
    /// can tell a `main` that reads `PORT` from one that binds a port of its own, which `PORT=0`
    /// would not show: an OS-chosen port and a hard-coded one look alike.
    pub fn server(exe: &str, envs: &[(&str, &str)]) -> Bin {
        let port = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port").local_addr().expect("its address").port();
        let port_text = port.to_string();
        let mut bin = Bin::spawn(exe, &[envs, &[("PORT", port_text.as_str())]].concat());
        bin.port = Some(port);
        bin
    }

    /// The next line the binary prints, or a panic when it prints none within [`GUARD`].
    pub fn next_line(&mut self) -> String {
        match self.lines.recv_timeout(GUARD) {
            Ok(line) => {
                self.seen.push(line.clone());
                line
            }
            Err(RecvTimeoutError::Timeout) => panic!("the binary printed nothing within {GUARD:?}; it printed {:?}", self.seen),
            Err(RecvTimeoutError::Disconnected) => panic!("the binary ended before printing a listening address; it printed {:?}", self.seen),
        }
    }

    /// Reads lines until the first `listening on http://ADDR` and returns `ADDR`. For a [`server`](Self::server)
    /// it panics when `ADDR` is not on the port that was asked for.
    pub fn addr(&mut self) -> SocketAddr {
        loop {
            let line = self.next_line();
            if let Some(addr) = line.strip_prefix("listening on http://") {
                let addr: SocketAddr = addr.parse().unwrap_or_else(|e| panic!("`{addr}` is not an address: {e}"));
                if let Some(port) = self.port {
                    assert_eq!(addr.port(), port, "the binary was given PORT={port} and listens on {addr}");
                }
                return addr;
            }
        }
    }

    /// Sends SIGTERM, waits for the exit and returns it.
    pub fn terminate(self) -> Exit {
        let status = Command::new("/bin/kill").args(["-TERM", &self.child.id().to_string()]).status().expect("kill runs");
        assert!(status.success(), "kill -TERM failed: {status}");
        self.finish()
    }

    /// Waits for the binary to end by itself and returns how.
    pub fn finish(mut self) -> Exit {
        let started = Instant::now();
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("the child can be polled") {
                break status;
            }
            assert!(started.elapsed() < GUARD, "the binary was still running {GUARD:?} later; it printed {:?}", self.seen);
            thread::sleep(Duration::from_millis(5));
        };
        let mut stdout = std::mem::take(&mut self.seen);
        // The reader thread ends at end-of-file, which follows the exit; wait for it so no line is lost.
        while let Ok(line) = self.lines.recv_timeout(GUARD) {
            stdout.push(line);
        }
        let stderr = self.stderr.take().expect("stderr is read once").join().expect("stderr reader joins");
        Exit { status, stdout, stderr }
    }
}

impl Drop for Bin {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            self.child.kill().ok();
            self.child.wait().ok();
        }
    }
}
