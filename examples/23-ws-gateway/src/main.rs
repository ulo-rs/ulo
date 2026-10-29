use ulo_runtime_tokio::{Tokio, shutdown_signal};
use ulo_ws_hyper::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let app = ex_23_ws_gateway::builder()
        .runtime(Tokio::current())
        .wire()?
        .connect()
        .await?
        .bind(Server::new(format!("127.0.0.1:{port}")))
        .listen()
        .await?;
    for bound in app.addresses() {
        println!("listening on http://{}", bound.addr);
    }
    let shutdown = app.serve(shutdown_signal()).await?;
    println!("closed on {}", shutdown.signal);
    Ok(())
}
