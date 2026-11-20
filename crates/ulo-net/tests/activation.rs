//! A descriptor inherited through the socket-activation protocol that is not a listening TCP
//! socket fails only the endpoint that names it. The process inheriting the descriptors is the
//! test binary run again as an ignored child test, `child_reads_a_mixed_set` or
//! `child_reads_every_kind`, `LISTEN_PID` set to its own process ID by the shell it is started
//! through.
#![cfg(unix)]

use std::fs::File;
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::os::fd::{OwnedFd, RawFd};
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};

use command_fds::{CommandFdExt, FdMapping};
use ulo_net::{Activation, ActivationError, ListenerName};

const CHILD: &str = "ULO_ACTIVATION_CHILD";

#[test]
#[cfg_attr(miri, ignore = "Miri refuses `socket` and `Command` under isolation")]
fn a_datagram_socket_beside_a_listener_fails_only_the_endpoint_naming_it() {
    let datagram = UdpSocket::bind("127.0.0.1:0").expect("a datagram socket");
    let stream = TcpListener::bind("127.0.0.1:0").expect("a listener");
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("LISTEN_PID=$$ exec \"$0\" \"$@\"")
        .arg(std::env::current_exe().expect("the test binary's path"))
        .args(["--exact", "child_reads_a_mixed_set", "--ignored", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("LISTEN_FDS", "2")
        .env("LISTEN_FDNAMES", "datagram:stream")
        .env_remove("LISTEN_PID")
        .env_remove("ULO_DEV")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let datagram: OwnedFd = datagram.into();
    let stream: OwnedFd = stream.into();
    command
        .fd_mappings(vec![
            FdMapping { parent_fd: datagram, child_fd: 3 },
            FdMapping { parent_fd: stream, child_fd: 4 },
        ])
        .expect("two distinct targets");

    let output = command.output().expect("the child runs");
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "the child failed:\n{text}");
    assert!(text.contains("1 passed"), "the child ran no test:\n{text}");
}

/// The child. Without `ULO_ACTIVATION_CHILD` it is an ordinary ignored test that does nothing.
#[test]
#[ignore = "run by the parent test, which hands it descriptors"]
fn child_reads_a_mixed_set() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let activation = Activation::get().expect("one datagram socket among the descriptors does not fail the set");
    let datagram = ListenerName::Named("datagram".into());
    let stream = ListenerName::Named("stream".into());

    assert_eq!(activation.count(&datagram), 0, "the datagram socket counted as a listener");
    assert!(
        matches!(activation.unavailable(&datagram), ActivationError::NotListening { index: 0 }),
        "prepare's reason for the datagram socket is wrong"
    );
    assert!(
        matches!(activation.take(&datagram), Err(ActivationError::NotListening { index: 0 })),
        "taking the datagram socket did not name it"
    );

    assert_eq!(activation.count(&stream), 1);
    let listener = activation.take(&stream).expect("the listener beside it is served");
    listener.local_addr().expect("an adopted listener");
}

/// The test binary run again as `child`, `fds` placed at 3 on, under `LISTEN_FDNAMES` `names`,
/// with `ports` for it to compare the adopted listeners with.
fn run_child(child: &str, fds: Vec<OwnedFd>, names: &str, ports: &[u16]) {
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("LISTEN_PID=$$ exec \"$0\" \"$@\"")
        .arg(std::env::current_exe().expect("the test binary's path"))
        .args(["--exact", child, "--ignored", "--nocapture", "--test-threads=1"])
        .env(CHILD, ports.iter().map(u16::to_string).collect::<Vec<_>>().join(":"))
        .env("LISTEN_FDS", fds.len().to_string())
        .env("LISTEN_FDNAMES", names)
        .env_remove("LISTEN_PID")
        .env_remove("ULO_DEV")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mappings = fds.into_iter().zip(3..).map(|(parent_fd, child_fd)| FdMapping { parent_fd, child_fd }).collect();
    command.fd_mappings(mappings).expect("distinct targets");
    let output = command.output().expect("the child runs");
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "the child failed:\n{text}");
    assert!(text.contains("1 passed"), "the child ran no test:\n{text}");
}

/// Whether descriptor `fd` is open in this process.
fn open(fd: RawFd) -> bool {
    std::fs::metadata(format!("/dev/fd/{fd}")).is_ok()
}

/// Which of `fds` a process this one spawns finds open: none, when each carries `FD_CLOEXEC`.
fn inherited_by_a_subprocess(fds: &[RawFd]) -> String {
    let list = fds.iter().map(RawFd::to_string).collect::<Vec<_>>().join(" ");
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("for fd in {list}; do [ -e /dev/fd/$fd ] && printf '%s ' $fd; done; true"))
        .output()
        .expect("the subprocess runs");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
#[cfg_attr(miri, ignore = "Miri refuses `socket` and `Command` under isolation")]
fn every_kind_of_descriptor_that_is_not_a_listener_is_refused_by_index_and_left_open() {
    let dir = std::env::temp_dir().join(format!("ulo-activation-kinds-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    let first = TcpListener::bind("127.0.0.1:0").expect("a listener");
    let (read, _write) = std::io::pipe().expect("a pipe");
    let datagram = UdpSocket::bind("127.0.0.1:0").expect("a datagram socket");
    let unix = UnixListener::bind(dir.join("listening.sock")).expect("a Unix listener");
    let file = File::create(dir.join("plain")).expect("a file");
    let middle = TcpListener::bind("127.0.0.1:0").expect("a listener");
    let last = TcpListener::bind("[::1]:0").expect("an IPv6 listener");
    let ports = [&first, &middle, &last].map(|listener| listener.local_addr().expect("its address").port());
    let fds = vec![first.into(), read.into(), datagram.into(), unix.into(), file.into(), middle.into(), last.into()];

    run_child("child_reads_every_kind", fds, "web:pipe:datagram:unix:file:admin:web", &ports);
    std::fs::remove_dir_all(&dir).expect("the directory goes");
}

/// The child of the test above: descriptors 3 to 9 a listener, a pipe, a datagram socket, a
/// listening Unix socket, a file, a listener, and an IPv6 listener named as the first is.
#[test]
#[ignore = "run by the parent test, which hands it descriptors"]
fn child_reads_every_kind() {
    let Ok(ports) = std::env::var(CHILD) else { return };
    let ports: Vec<u16> = ports.split(':').map(|port| port.parse().expect("a port")).collect();
    let activation = Activation::get().expect("descriptors that are not listeners do not fail the set");
    let range: Vec<RawFd> = (3..10).collect();
    for &fd in &range {
        assert!(open(fd), "descriptor {fd} is closed after `get`");
    }
    assert_eq!(inherited_by_a_subprocess(&range), "", "descriptors a subprocess inherits");

    for (name, index) in [("pipe", 1), ("datagram", 2), ("unix", 3), ("file", 4)] {
        let named = ListenerName::Named(name.into());
        assert_eq!(activation.count(&named), 0, "`{name}` counted as a listener");
        let reason = activation.unavailable(&named);
        assert!(matches!(reason, ActivationError::NotListening { index: at } if at == index), "`{name}`: {reason:?}");
        let reason = activation.unavailable(&ListenerName::Index(index));
        assert!(matches!(reason, ActivationError::NotListening { index: at } if at == index), "#{index}: {reason:?}");
    }
    let absent = activation.unavailable(&ListenerName::Named("absent".into()));
    assert!(matches!(absent, ActivationError::Missing(_)), "{absent:?}");

    let web = ListenerName::Named("web".into());
    let port = |listener: &TcpListener| listener.local_addr().expect("an adopted listener").port();
    let first = activation.take(&web).expect("the first `web`");
    let second = activation.take(&web).expect("the second `web`");
    assert_eq!((port(&first), port(&second)), (ports[0], ports[2]), "`web` answers for #0, then #6");
    let reason = activation.take(&web).expect_err("both `web` sockets are taken");
    assert!(matches!(reason, ActivationError::Missing(_)), "{reason:?}");
    let admin = activation.take(&ListenerName::Index(5)).expect("#5, the listener between the others");
    assert_eq!(port(&admin), ports[1]);
    let _client = TcpStream::connect(("127.0.0.1", ports[1])).expect("the adopted listener accepts");
    admin.set_nonblocking(false).expect("blocking");
    admin.accept().expect("the connection");

    drop((first, second, admin));
    for fd in [3, 8, 9] {
        assert!(!open(fd), "adopted descriptor {fd} is open after its listener dropped");
    }
    for fd in [4, 5, 6, 7] {
        assert!(open(fd), "descriptor {fd}, never adopted, was closed");
    }
}
