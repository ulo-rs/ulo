use ulo_runtime_tokio::{Tokio, shutdown_signal};

/// `RPC_LINK` names the link; without it the handlers listen on TCP at `PORT`.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "7000".into());
    let url = std::env::var("RPC_LINK").unwrap_or_else(|_| format!("tcp://127.0.0.1:{port}"));
    let app = ex_28_rpc_links::bind(ex_28_rpc_links::builder().runtime(Tokio::current()).wire()?.connect().await?, &url)?
        .listen()
        .await?;
    println!("serving over {url}");
    for bound in app.addresses() {
        println!("listening on tcp://{}", bound.addr);
    }
    app.serve(shutdown_signal()).await?;
    Ok(())
}
