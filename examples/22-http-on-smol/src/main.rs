fn main() -> Result<(), ulo::BoxError> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    ulo_runtime_smol::run(ex_22_http_on_smol::AppModule, ex_22_http_on_smol::server(&format!("127.0.0.1:{port}")))?;
    Ok(())
}
