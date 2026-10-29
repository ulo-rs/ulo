use std::path::Path;

use ulo_config::{Env, Values};
use ulo_http_hyper::Server;
use ulo_runtime_tokio::{Tokio, shutdown_signal};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = std::env::var("PORT").unwrap_or_else(|_| "3000".into()).parse()?;
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("config");
    // The process environment is read here and nowhere else: no source finds it by itself.
    let config = ex_33_config::config(&dir, Env::process(), Values::new().set("http.host", "127.0.0.1").set("http.port", port));
    let app = ex_33_config::builder(config).runtime(Tokio::current()).wire()?.connect().await?;
    let settings = app.get::<ex_33_config::AppConfig>().await?;
    let app = app.bind(Server::new(format!("{}:{}", settings.http.host, settings.http.port))).listen().await?;
    for bound in app.addresses() {
        println!("listening on http://{}", bound.addr);
    }
    app.serve(shutdown_signal()).await?;
    Ok(())
}
