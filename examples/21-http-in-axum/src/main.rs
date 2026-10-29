use ulo_runtime_tokio::{Tokio, shutdown_signal};

#[tokio::main]
async fn main() -> Result<(), ulo::BoxError> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let server = ex_21_http_in_axum::embedded();
    let embedded = server.handle();
    let app = ex_21_http_in_axum::builder().runtime(Tokio::current()).wire()?.connect().await?.bind(server).listen().await?;

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    println!("listening on http://{}", listener.local_addr()?);
    let host = axum::serve(listener, ex_21_http_in_axum::router(&embedded));
    ulo_http_axum::run(app, &embedded, host, shutdown_signal()).await?;
    Ok(())
}
