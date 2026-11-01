use ulo_runtime_tokio::{Tokio, shutdown_signal};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let activated = std::env::var_os("LISTEN_FDS").is_some();
    let app = ex_40_socket_activation::builder()
        .runtime(Tokio::current())
        .wire()?
        .connect()
        .await?
        .bind(ex_40_socket_activation::server(activated, &port))
        .listen()
        .await?;
    for bound in app.addresses() {
        println!("listening on http://{}", bound.addr);
    }
    app.serve(shutdown_signal()).await?;
    Ok(())
}
