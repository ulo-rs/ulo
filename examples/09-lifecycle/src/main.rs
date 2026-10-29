use ex_09_lifecycle::{Journal, builder};
use ulo::Signal;
use ulo_runtime_tokio::Tokio;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = builder(2).runtime(Tokio::current()).wire()?.connect().await?;
    let journal = app.get::<Journal>().await?;
    app.close(Signal::new("done")).await?;
    for line in journal.lines() {
        println!("{line}");
    }
    Ok(())
}
