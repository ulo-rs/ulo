use ulo_grpc::Server;
use ulo_runtime_tokio::{Tokio, shutdown_signal};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "50051".into());
    let app = ex_29_grpc_service::builder()
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
    app.serve(shutdown_signal()).await?;
    Ok(())
}
