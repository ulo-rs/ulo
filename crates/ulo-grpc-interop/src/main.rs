use std::path::Path;

use ulo_grpc::Server;
use ulo_grpc::config::Tls;
use ulo_runtime_tokio::{Tokio, shutdown_signal};

/// The name the interop client checks the server's certificate against unless told another:
/// grpc-go's `--server_host_override` default.
const TEST_HOST: &str = "foo.test.google.fr";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [command, dir] = args.as_slice()
        && command == "certs"
    {
        return write_certs(Path::new(dir));
    }
    if !args.is_empty() {
        return Err(format!("usage: ulo-grpc-interop [certs DIR], got {args:?}").into());
    }
    let port = std::env::var("PORT").unwrap_or_else(|_| "10000".into());
    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let mut server = Server::new(format!("{host}:{port}")).reflection(false);
    let tls = match (std::env::var_os("TLS_CERT"), std::env::var_os("TLS_KEY")) {
        (Some(cert), Some(key)) => Some(Tls::from_pem_files(cert, key)),
        (None, None) => None,
        _ => return Err("TLS_CERT and TLS_KEY are set together or not at all".into()),
    };
    let scheme = if tls.is_some() { "https" } else { "http" };
    if let Some(tls) = tls {
        server = server.tls(tls);
    }
    let app = ulo_grpc_interop::builder().runtime(Tokio::current()).wire()?.connect().await?.bind(server).listen().await?;
    for bound in app.addresses() {
        println!("listening on {scheme}://{}", bound.addr);
    }
    app.serve(shutdown_signal()).await?;
    Ok(())
}

/// A test certificate authority and a server certificate it issued for [`TEST_HOST`], `localhost`
/// and `127.0.0.1`, written to `dir` as `ca.pem`, `cert.pem` and `key.pem`: generated per run, so
/// no private key is kept anywhere.
fn write_certs(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let ca = ulo_test_certs::Ca::new();
    let server = ca.issue(&[TEST_HOST, "localhost", "127.0.0.1"]);
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("ca.pem"), ca.cert_pem())?;
    std::fs::write(dir.join("cert.pem"), server.cert_pem())?;
    std::fs::write(dir.join("key.pem"), server.key_pem())?;
    Ok(())
}
