use ulo_http_hyper::Server as Http;
use ulo_rpc::Server as Rpc;
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::{Tokio, shutdown_signal};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let http = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let rpc = std::env::var("RPC_PORT").unwrap_or_else(|_| "7000".into());
    let app = ex_41_one_guard_two_transports::builder()
        .runtime(Tokio::current())
        .wire()?
        .connect()
        .await?
        .bind(Http::new(format!("127.0.0.1:{http}")))
        .bind(Rpc::new(Tcp::new(format!("127.0.0.1:{rpc}"))))
        .listen()
        .await?;
    for bound in app.addresses() {
        let scheme = if bound.transport == "rpc" { "tcp" } else { "http" };
        println!("listening on {scheme}://{}", bound.addr);
    }
    app.serve(shutdown_signal()).await?;
    Ok(())
}
