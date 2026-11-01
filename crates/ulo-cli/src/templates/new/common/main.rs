@@uses@@

@@modules@@

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

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
