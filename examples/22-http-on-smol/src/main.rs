fn main() -> Result<(), ulo::BoxError> {
    // Prints the `listening` event `run` logs for the address. The builder logs at `info` in any
    // build, where `fmt::init()` prints errors alone when a dependency enables `env-filter` and
    // `RUST_LOG` is unset.
    tracing_subscriber::fmt().init();
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    ulo_runtime_smol::run(ex_22_http_on_smol::AppModule, ex_22_http_on_smol::server(&format!("127.0.0.1:{port}")))?;
    Ok(())
}
