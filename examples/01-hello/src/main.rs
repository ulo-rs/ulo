use ulo_http_hyper::Server;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    ulo_runtime_tokio::run(ex_01_hello::AppModule, Server::new(format!("127.0.0.1:{port}")))?;
    Ok(())
}
