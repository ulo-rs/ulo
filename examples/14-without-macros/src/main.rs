use ex_14_without_macros::{BillingModule, InvoiceService, builder};
use ulo::Signal;
use ulo_runtime_tokio::Tokio;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = builder().runtime(Tokio::current()).wire()?.connect().await?;
    println!("{}", app.module::<BillingModule>()?.get::<InvoiceService>().await?.open_for("ada"));
    app.close(Signal::new("done")).await?;
    Ok(())
}
