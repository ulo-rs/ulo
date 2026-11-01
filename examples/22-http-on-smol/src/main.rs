use std::sync::Arc;

use async_signal::{Signal as Os, Signals};
use futures_util::StreamExt;
use ulo::Signal;
use ulo_runtime_smol::{Executor, Smol};

/// Resolves on the first SIGINT or SIGTERM.
async fn shutdown_signal() -> Signal {
    let mut signals = Signals::new([Os::Int, Os::Term]).expect("the signal handlers install");
    match signals.next().await {
        Some(Ok(Os::Term)) => Signal::new("SIGTERM"),
        _ => Signal::new("SIGINT"),
    }
}

fn main() -> Result<(), ulo::BoxError> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let smol = Smol::new(Arc::new(Executor::new()));
    smol.block_on(async {
        let app = ex_22_http_on_smol::builder()
            .runtime(smol.clone())
            .wire()?
            .connect()
            .await?
            .bind(ex_22_http_on_smol::server(&format!("127.0.0.1:{port}")))
            .listen()
            .await?;
        for bound in app.addresses() {
            println!("listening on http://{}", bound.addr);
        }
        app.serve(shutdown_signal()).await?;
        Ok(())
    })
}
