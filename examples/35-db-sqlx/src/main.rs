use ulo_config::{Env, Values};
use ulo_http_hyper::Server;
use ulo_runtime_tokio::{Tokio, shutdown_signal};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let config = ex_35_db_sqlx::config(Env::process(), Values::new());
    let app = ex_35_db_sqlx::builder(config)
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
