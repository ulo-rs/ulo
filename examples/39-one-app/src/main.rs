use ex_39_one_app::{AppConfig, Store, builder, config};
use ulo_config::{Env, Values};
use ulo_grpc::Server as Grpc;
use ulo_http_hyper::Server as Http;
use ulo_rpc::Server as Rpc;
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::{Tokio, shutdown_signal};
use ulo_ws_hyper::Server as Ws;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = std::env::var("PORT").unwrap_or_else(|_| "3000".into()).parse()?;
    let config = config(Env::process(), Values::new().set("ports.http", port));
    let connected = builder(config).runtime(Tokio::current()).wire()?.connect().await?;
    let ports = connected.get::<AppConfig>().await?;
    let store = connected.get::<Store>().await?;
    let host = &ports.host;
    let app = connected
        .bind(Http::new(format!("{host}:{}", ports.ports.http)))
        .bind(Ws::new(format!("{host}:{}", ports.ports.ws)))
        .bind(Rpc::new(Tcp::new(format!("{host}:{}", ports.ports.rpc))))
        .bind(Grpc::new(format!("{host}:{}", ports.ports.grpc)))
        .listen()
        .await?;
    for bound in app.addresses() {
        let scheme = match bound.transport {
            "ws" => "ws",
            "rpc" => "tcp",
            "grpc" => "grpc",
            _ => "http",
        };
        println!("listening on {scheme}://{}", bound.addr);
    }
    store.open();
    println!("store open");
    let shutdown = app.serve(shutdown_signal()).await?;
    println!("closed on {}", shutdown.signal);
    Ok(())
}
