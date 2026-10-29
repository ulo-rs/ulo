use ex_12_standalone::{builder, report};
use ulo::Signal;
use ulo_runtime_tokio::Tokio;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = builder().runtime(Tokio::current()).wire()?.connect().await?;
    println!("{}", report(&app, "acme").await??);
    let shutdown = app.close(Signal::new("done")).await?;
    println!("closed on {}", shutdown.signal);
    Ok(())
}
