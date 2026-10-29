use ulo::Signal;
use ulo_grpc::Server;
use ulo_runtime_tokio::{Tokio, shutdown_signal};

/// `CALLEE` names a server of `clock.v1.Clock`; without it the binary serves one of its own on a
/// free port and the front calls that.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "50052".into());
    let (callee, own) = match std::env::var("CALLEE") {
        Ok(callee) => (callee, None),
        Err(_) => {
            let clock = ex_30_grpc_client::callee_builder(ex_30_grpc_client::Calls::default())
                .runtime(Tokio::current())
                .wire()?
                .connect()
                .await?
                .bind(Server::new("127.0.0.1:0"))
                .listen()
                .await?;
            let origin = format!("http://{}", clock.addresses()[0].addr);
            let handle = clock.handle();
            (origin, Some((handle, tokio::spawn(clock.serve(std::future::pending::<Signal>())))))
        }
    };
    let app = ex_30_grpc_client::front_builder(callee)
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
    if let Some((handle, serving)) = own {
        handle.close(Signal::new("the front closed")).await?;
        serving.await??;
    }
    Ok(())
}
