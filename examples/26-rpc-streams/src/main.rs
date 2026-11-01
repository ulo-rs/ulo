use ulo_rpc::Server;
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::{Tokio, shutdown_signal};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "7000".into());
    let app = ex_26_rpc_streams::builder()
        .runtime(Tokio::current())
        .wire()?
        .connect()
        .await?
        .bind(Server::new(Tcp::new(format!("127.0.0.1:{port}"))))
        .listen()
        .await?;
    for bound in app.addresses() {
        println!("listening on tcp://{}", bound.addr);
    }
    app.serve(shutdown_signal()).await?;
    Ok(())
}
