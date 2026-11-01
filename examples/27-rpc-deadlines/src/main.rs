use ulo::Signal;
use ulo_rpc::Server;
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::{Tokio, shutdown_signal};

/// The callee listens on a port of its own and the gateway, on `PORT`, calls it.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "7000".into());
    let callee = ex_27_rpc_deadlines::callee()
        .runtime(Tokio::current())
        .wire()?
        .connect()
        .await?
        .bind(Server::new(Tcp::new("127.0.0.1:0")))
        .listen()
        .await?;
    let callee_addr = callee.addresses()[0].addr;
    let callee_handle = callee.handle();
    let callee = tokio::spawn(callee.serve(std::future::pending::<Signal>()));
    let gateway = ex_27_rpc_deadlines::gateway(callee_addr)
        .runtime(Tokio::current())
        .wire()?
        .connect()
        .await?
        .bind(Server::new(Tcp::new(format!("127.0.0.1:{port}"))))
        .listen()
        .await?;
    for bound in gateway.addresses() {
        println!("listening on tcp://{}", bound.addr);
    }
    gateway.serve(shutdown_signal()).await?;
    callee_handle.close(Signal::new("the gateway closed")).await?;
    callee.await??;
    Ok(())
}
