use ulo_runtime_tokio::{Tokio, shutdown_signal};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let cert = std::env::var("TLS_CERT").map_err(|_| "set TLS_CERT to a PEM certificate chain file")?;
    let key = std::env::var("TLS_KEY").map_err(|_| "set TLS_KEY to the PEM private key file")?;
    let app = ex_20_http_tls::builder()
        .runtime(Tokio::current())
        .wire()?
        .connect()
        .await?
        .bind(ex_20_http_tls::server(&format!("127.0.0.1:{port}"), &cert, &key))
        .listen()
        .await?;
    for bound in app.addresses() {
        println!("listening on https://{}", bound.addr);
    }
    app.serve(shutdown_signal()).await?;
    Ok(())
}
