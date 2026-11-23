@@uses@@

@@modules@@

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `RUST_LOG` when it is set and parses, `info` otherwise, so a plain `cargo run` prints the
    // "listening" line `run` writes for each address.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // `load` never fails: a source that cannot be read is recorded in the value and reported by
    // `wire()` with every other problem. The server `run` takes reads its `@@key@@` here; a
    // configuration that does not read leaves it empty, and `run` stops at `wire`, which reads
    // `AppConfig` again through `ConfigModule` and reports it, before any server is prepared.
    let config = Config::builder()
        .source(File::toml("config/app.toml"))
        .source(Env::process().prefix("APP_").separator("__"))
        .load();
    let @@key@@ = Section::<AppConfig>::read(&config, "")
        .value()
        .map(|settings| settings.server.@@key@@.clone())
        .unwrap_or_default();

    // Serves until Ctrl-C or SIGTERM. An application that sets a builder knob writes the phases
    // `run` performs: `App::builder(..).runtime(..).wire()`, `connect`, `bind`, `listen`, `serve`.
    ulo_runtime_tokio::run(AppModule::new(config), @@server@@)?;
    Ok(())
}
