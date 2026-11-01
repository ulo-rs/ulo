//! Servers that answer only the opening of a connect, for the store tests that need a TLS client to
//! reach the point where it reads its certificate files and no further. No database runs.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

/// Answers each connection's Postgres `SSLRequest` with `S` and holds the connection open.
pub fn postgres_ssl() -> u16 {
    serve(|stream| {
        let mut request = [0u8; 8];
        stream.read_exact(&mut request).is_ok() && stream.write_all(b"S").is_ok()
    })
}

/// A MySQL 8.0.18 initial handshake whose capabilities include `CLIENT_SSL`, the packet sqlx-mysql's
/// own decoder test carries.
const MYSQL_HANDSHAKE: &[u8] = b"\n8.0.18\x00\x19\x00\x00\x00\x114aB0c\x06g\x00\xff\xff\xff\x02\x00\xff\xc7\x15\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00tL\x03s\x0f[4\rl4. \x00caching_sha2_password\x00";

/// Sends each connection a MySQL handshake offering TLS, reads the client's `SSLRequest` and holds
/// the connection open.
pub fn mysql_tls() -> u16 {
    serve(|stream| {
        let len = MYSQL_HANDSHAKE.len() as u32;
        let mut packet = vec![len as u8, (len >> 8) as u8, (len >> 16) as u8, 0];
        packet.extend_from_slice(MYSQL_HANDSHAKE);
        let mut request = [0u8; 36];
        stream.write_all(&packet).is_ok() && stream.read_exact(&mut request).is_ok()
    })
}

/// A loopback listener on a thread of its own, running `open` on every connection and then
/// holding it until the client closes it or five seconds pass.
fn serve(open: fn(&mut std::net::TcpStream) -> bool) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
    let port = listener.local_addr().expect("an address").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            std::thread::spawn(move || {
                if open(&mut stream) {
                    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
                    let mut rest = [0u8; 64];
                    let _ = stream.read(&mut rest);
                }
            });
        }
    });
    port
}
