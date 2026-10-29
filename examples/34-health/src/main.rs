use ex_34_health::Cache;
use ulo_http_hyper::Server;
use ulo_runtime_tokio::{Tokio, shutdown_signal};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let grpc_port = std::env::var("GRPC_PORT").unwrap_or_else(|_| "50051".into());
    let connected = ex_34_health::builder().runtime(Tokio::current()).wire()?.connect().await?;
    let cache = connected.get::<Cache>().await?;
    let app = connected
        .bind(Server::new(format!("127.0.0.1:{port}")))
        .bind(ulo_grpc::Server::new(format!("127.0.0.1:{grpc_port}")))
        .listen()
        .await?;
    for bound in app.addresses() {
        println!("listening on http://{}", bound.addr);
    }
    // The probes answer from the moment the app listens; ready stays 503 until this line has run.
    cache.warm_up();
    println!("cache warm");
    app.serve(shutdown_signal()).await?;
    Ok(())
}
