//! `run` as a `main` uses it. A child process of this test binary serves an app through `run` on a
//! real socket and the parent ends it with SIGTERM, reading what the child printed: the shutdown
//! hooks and the server's `close` ran, in the sequence's order, and `run` answered the signal. In
//! this process, a startup that fails, a port already held or an init hook's error, answers
//! `RunError::Startup` and serves nothing.
#![expect(clippy::result_large_err, reason = "`bounded` answers what `run` answers, `RunError` holding the core's `StartupError` unchanged")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use ulo::{BoundAddr, BoxError, DrainToken, Module, ModuleDef, ModuleIdentity, Mounted, Server, Signal, StartupError, Transport};
use ulo_runtime_tokio::{RunError, run};

/// The most real time any wait on `run` or on the child may take.
const GUARD: Duration = Duration::from_secs(10);

/// Set in the child's environment: the `child` test serves only when it finds it.
const CHILD: &str = "ULO_RUN_CHILD";

struct Greeting;

impl Transport for Greeting {
    const KEY: &'static str = "greeting";
    type Cx = ();
    type Reply = ();
}

/// What a [`Greeter`] reached.
#[derive(Default)]
struct Reached {
    bind: AtomicBool,
    serve: AtomicBool,
}

/// A server writing `hello` to each connection it accepts on a TCP listener, printing each step a
/// parent reads.
struct Greeter {
    endpoint: SocketAddr,
    addr: Option<SocketAddr>,
    listener: Mutex<Option<TcpListener>>,
    reached: Arc<Reached>,
}

impl Greeter {
    fn new(endpoint: SocketAddr) -> (Greeter, Arc<Reached>) {
        let reached = Arc::new(Reached::default());
        (Greeter { endpoint, addr: None, listener: Mutex::new(None), reached: Arc::clone(&reached) }, reached)
    }
}

impl Server for Greeter {
    type Transport = Greeting;

    async fn bind(&mut self, _mounted: Mounted<'_, Greeting>) -> Result<(), BoxError> {
        self.reached.bind.store(true, Ordering::SeqCst);
        let listener = TcpListener::bind(self.endpoint)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        println!("bound {addr}");
        self.addr = Some(addr);
        *self.listener.get_mut().unwrap_or_else(PoisonError::into_inner) = Some(listener);
        Ok(())
    }

    /// `serve` polls the signal future before any server's, and the body runs at the first poll,
    /// so `serving` is printed after the signal handlers are installed: a SIGTERM sent once it is
    /// read reaches them rather than the default action.
    async fn serve(&self) -> Result<(), BoxError> {
        self.reached.serve.store(true, Ordering::SeqCst);
        println!("serving");
        let listener = self.listener.lock().unwrap_or_else(PoisonError::into_inner).take().ok_or("`serve` ran twice")?;
        let listener = tokio::net::TcpListener::from_std(listener)?;
        loop {
            let (stream, _) = listener.accept().await?;
            stream.writable().await?;
            stream.try_write(b"hello\n")?;
        }
    }

    async fn drain(&self, _token: DrainToken) {}

    async fn close(&self) -> Result<(), BoxError> {
        println!("closed");
        Ok(())
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.addr.map(|addr| BoundAddr::new(Greeting::KEY, addr)).into_iter().collect()
    }
}

/// A module whose two shutdown hooks print the signal they receive.
struct Hooked;

impl Module for Hooked {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.before_shutdown(|signal: Signal| async move { println!("before_shutdown {signal}") });
        m.on_shutdown(|signal: Signal| async move { println!("on_shutdown {signal}") });
    }
}

/// A module whose init hook fails.
struct Refusing;

impl Module for Refusing {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.on_init(|| async { Err::<(), _>(std::io::Error::other("the store is unreachable")) });
    }
}

/// A module whose init hook prints `initializing` and then waits for a line on stdin, holding
/// `connect` until the parent has sent its signal.
struct Held;

impl Module for Held {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(Hooked);
        m.on_init(|| async {
            println!("initializing");
            std::io::stdin().read_line(&mut String::new()).map(drop)
        });
    }
}

/// A module whose init hook starts a blocking task that outlives any drain.
struct Stuck;

impl Module for Stuck {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(Hooked);
        m.on_init(|| async {
            drop(tokio::task::spawn_blocking(|| thread::sleep(Duration::from_secs(600))));
            Ok::<(), std::io::Error>(())
        });
    }
}

/// The child's side of the tests that signal a child: run by them as a process of its own, and a
/// pass when the binary runs as a whole. `CHILD` set to `held` serves [`Held`], to `stuck`
/// [`Stuck`], otherwise [`Hooked`].
#[test]
fn child() {
    let Some(mode) = std::env::var_os(CHILD) else {
        return;
    };
    tracing_subscriber::fmt().with_writer(std::io::stdout).init();
    let (server, _) = Greeter::new(SocketAddr::from(([127, 0, 0, 1], 0)));
    let answer = if mode == "held" {
        run(Held, server)
    } else if mode == "stuck" {
        run(Stuck, server)
    } else {
        run(Hooked, server)
    };
    match answer {
        Ok(shutdown) => println!("returned {}", shutdown.signal),
        Err(error) => println!("failed {error}"),
    }
}

/// This test binary run again as the `child` test, its stdout read line by line. Dropping it kills
/// the process, so a failed assertion leaves nothing listening.
struct Spawned {
    child: Child,
    lines: Receiver<String>,
    seen: Vec<String>,
}

impl Spawned {
    fn start(mode: &str) -> Spawned {
        let exe = std::env::current_exe().expect("the test binary's path");
        let mut child = Command::new(exe)
            .args(["--exact", "child", "--nocapture", "--test-threads=1"])
            .env(CHILD, mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the test binary starts again");
        let stdout = child.stdout.take().expect("stdout is piped");
        let (send, lines) = channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Spawned { child, lines, seen: Vec::new() }
    }

    /// Reads lines until one holding `prefix` and answers the rest of it. The test harness writes
    /// `test child ... ` with no line end, so the child's first line follows it.
    fn after(&mut self, prefix: &str) -> String {
        loop {
            match self.lines.recv_timeout(GUARD) {
                Ok(line) => {
                    self.seen.push(line.clone());
                    if let Some((_, rest)) = line.split_once(prefix) {
                        return rest.to_owned();
                    }
                }
                Err(RecvTimeoutError::Timeout) => panic!("the child printed no `{prefix}` line within {GUARD:?}; it printed {:?}", self.seen),
                Err(RecvTimeoutError::Disconnected) => panic!("the child ended before printing `{prefix}`; it printed {:?}", self.seen),
            }
        }
    }

    fn terminate(&self) {
        let status = Command::new("/bin/kill").args(["-TERM", &self.child.id().to_string()]).status().expect("kill runs");
        assert!(status.success(), "kill -TERM failed: {status}");
    }

    /// Waits for the child to end and answers how, with every line it printed.
    fn finish(mut self) -> (ExitStatus, Vec<String>) {
        let started = Instant::now();
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("the child can be polled") {
                break status;
            }
            assert!(started.elapsed() < GUARD, "the child was still running {GUARD:?} after SIGTERM; it printed {:?}", self.seen);
            thread::sleep(Duration::from_millis(5));
        };
        // The reader ends at end-of-file, which follows the exit.
        while let Ok(line) = self.lines.recv_timeout(GUARD) {
            self.seen.push(line);
        }
        (status, std::mem::take(&mut self.seen))
    }
}

impl Drop for Spawned {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            self.child.kill().ok();
            self.child.wait().ok();
        }
    }
}

/// The position of the first line ending in `wanted`, or a failure listing every line.
fn position(lines: &[String], wanted: &str) -> usize {
    lines.iter().position(|line| line.ends_with(wanted)).unwrap_or_else(|| panic!("no `{wanted}` line; the child printed {lines:?}"))
}

#[cfg(unix)]
#[test]
fn run_serves_until_sigterm_then_shuts_down_and_answers_the_signal() {
    let mut child = Spawned::start("serve");
    let addr: SocketAddr = child.after("bound ").parse().expect("the child printed an address");
    child.after("serving");

    let mut stream = TcpStream::connect_timeout(&addr, GUARD).expect("the served address accepts");
    stream.set_read_timeout(Some(GUARD)).expect("a read timeout");
    let mut greeting = String::new();
    stream.read_to_string(&mut greeting).expect("the server answered");
    assert_eq!(greeting, "hello\n", "what the server wrote to a connection");

    child.terminate();
    let (status, lines) = child.finish();
    assert!(status.success(), "the child ended {status} after SIGTERM; it printed {lines:?}");
    let listening = lines.iter().any(|line| line.contains("listening") && line.contains("ulo.transport=\"greeting\"") && line.contains(&addr.to_string()));
    assert!(listening, "no `listening` log line naming the transport and {addr}; the child printed {lines:?}");
    let order = ["before_shutdown SIGTERM", "closed", "on_shutdown SIGTERM", "returned SIGTERM"].map(|wanted| position(&lines, wanted));
    assert!(order.is_sorted(), "the shutdown ran out of order, at lines {order:?}; the child printed {lines:?}");
}

/// A signal sent while `connect` runs an init hook is held, not acted on by default: the app
/// finishes starting, then shuts down through its hooks, and `run` answers the signal.
#[cfg(unix)]
#[test]
fn a_sigterm_during_connect_shuts_the_started_app_down() {
    let mut child = Spawned::start("held");
    child.after("initializing");
    child.terminate();
    // A child the signal ended has closed its stdin, and the write fails.
    let _ = writeln!(child.child.stdin.as_mut().expect("stdin is piped"));
    let (status, lines) = child.finish();
    assert!(status.success(), "the child ended {status} after SIGTERM during `connect`; it printed {lines:?}");
    let order = ["before_shutdown SIGTERM", "closed", "on_shutdown SIGTERM", "returned SIGTERM"].map(|wanted| position(&lines, wanted));
    assert!(order.is_sorted(), "the shutdown ran out of order, at lines {order:?}; the child printed {lines:?}");
}

/// A blocking task still running once the app has shut down does not hold `run`: the child
/// answers the signal and exits within [`GUARD`], where the task sleeps for ten minutes.
#[cfg(unix)]
#[test]
fn a_blocking_task_still_running_does_not_hold_run_after_shutdown() {
    let mut child = Spawned::start("stuck");
    child.after("serving");
    child.terminate();
    let (status, lines) = child.finish();
    assert!(status.success(), "the child ended {status} after SIGTERM; it printed {lines:?}");
    position(&lines, "returned SIGTERM");
}

/// `run` on a thread of its own, its answer awaited for [`GUARD`] at most: a `run` that served
/// would wait for a signal no test sends.
fn bounded(start: impl FnOnce() -> Result<ulo::Shutdown, RunError> + Send + 'static) -> Result<ulo::Shutdown, RunError> {
    let (send, answer) = channel();
    thread::spawn(move || {
        let _ = send.send(start());
    });
    match answer.recv_timeout(GUARD) {
        Ok(answer) => answer,
        Err(RecvTimeoutError::Timeout) => panic!("`run` did not return within {GUARD:?}"),
        Err(RecvTimeoutError::Disconnected) => panic!("`run` panicked"),
    }
}

#[test]
fn a_port_already_held_is_a_startup_error_and_nothing_serves() {
    let held = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let (server, reached) = Greeter::new(held.local_addr().expect("its address"));
    let answer = bounded(move || run(Hooked, server));
    assert!(matches!(answer, Err(RunError::Startup(StartupError::Bind { .. }))), "`run` answered {answer:?}");
    assert!(reached.bind.load(Ordering::SeqCst), "the server's `bind` did not run");
    assert!(!reached.serve.load(Ordering::SeqCst), "the server served after its `bind` failed");
}

/// `listen` binds in queue order and stops at the first failure, so the first server binding and
/// the second refusing shows both were queued, first to last.
#[test]
fn a_tuple_of_servers_binds_each_in_the_order_written() {
    let held = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let (first, first_reached) = Greeter::new(SocketAddr::from(([127, 0, 0, 1], 0)));
    let (second, second_reached) = Greeter::new(held.local_addr().expect("its address"));
    let answer = bounded(move || run(Hooked, (first, second)));
    assert!(matches!(answer, Err(RunError::Startup(StartupError::Bind { .. }))), "`run` answered {answer:?}");
    assert!(first_reached.bind.load(Ordering::SeqCst), "the first server was not bound before the second");
    assert!(second_reached.bind.load(Ordering::SeqCst), "the second server was not bound");
    assert!(!first_reached.serve.load(Ordering::SeqCst), "the first server served after the second failed to bind");
}

#[test]
fn an_init_hook_s_error_is_a_startup_error_and_nothing_binds() {
    let (server, reached) = Greeter::new(SocketAddr::from(([127, 0, 0, 1], 0)));
    let answer = bounded(move || run(Refusing, server));
    match &answer {
        Err(RunError::Startup(error @ StartupError::Connect { .. })) => {
            assert!(error.to_string().contains("the store is unreachable"), "the startup error lost the hook's: {error}");
        }
        other => panic!("`run` answered {other:?}"),
    }
    assert!(!reached.bind.load(Ordering::SeqCst), "the server bound after an init hook failed");
    assert!(!reached.serve.load(Ordering::SeqCst), "the server served after an init hook failed");
}
