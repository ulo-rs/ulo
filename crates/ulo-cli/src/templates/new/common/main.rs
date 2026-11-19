@@uses@@

@@modules@@

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `RUST_LOG` when it is set and parses, `info` otherwise, so a plain `cargo run` prints the
    // "listening" line below.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // `load` never fails: a source that cannot be read is recorded in the value and reported by
    // `wire()` with every other problem.
    let config = Config::builder()
        .source(File::toml("config/app.toml"))
        .source(Env::process().prefix("APP_").separator("__"))
        .load();

    let app = App::builder(AppModule::new(config))
        .runtime(Tokio::current())
        .wire()?
        .connect()
        .await?;
    let settings = app.get::<AppConfig>().await?;
    let app = app.bind(@@server@@).listen().await?;
    for bound in app.addresses() {
        tracing::info!(address = %bound.addr, "listening");
    }
    app.serve(shutdown_signal()).await?;
    Ok(())
}
