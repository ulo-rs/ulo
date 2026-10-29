use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use ulo::Signal;
use ulo_config::Env;
use ulo_otel::{Otel, OtelOptions};
use ulo_otel_otlp::OtlpExporter;
use ulo_runtime_tokio::{Tokio, shutdown_signal};

/// The three services of the chain in one process, each its own app on its own port; the edge
/// listens on `PORT` and the other two on ports the system picks.
#[tokio::main]
async fn main() -> Result<(), ulo::BoxError> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    // `OTEL_EXPORTER_OTLP_ENDPOINT` names the collector; unset, spans go to http://localhost:4318.
    let otel = Otel::new(OtelOptions::from_env(&Env::process())?, OtlpExporter::http())?;
    // h2 and hyper open trace-level spans; the layer exports whatever the subscriber lets through.
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(LevelFilter::INFO).with(otel.layer()))?;

    let pricing = ex_38_tracing::pricing(&otel)
        .runtime(Tokio::current())
        .wire()?
        .connect()
        .await?
        .bind(ulo_grpc::Server::new("127.0.0.1:0"))
        .listen()
        .await?;
    let pricing_addr = pricing.addresses()[0].addr;
    let pricing_handle = pricing.handle();
    let pricing = tokio::spawn(pricing.serve(std::future::pending::<Signal>()));

    let stock = ex_38_tracing::stock(&otel, pricing_addr)
        .runtime(Tokio::current())
        .wire()?
        .connect()
        .await?
        .bind(ulo_rpc::Server::new(ulo_rpc_tcp::Tcp::new("127.0.0.1:0")))
        .listen()
        .await?;
    let stock_addr = stock.addresses()[0].addr;
    let stock_handle = stock.handle();
    let stock = tokio::spawn(stock.serve(std::future::pending::<Signal>()));

    let edge = ex_38_tracing::edge(&otel, stock_addr)
        .runtime(Tokio::current())
        .wire()?
        .connect()
        .await?
        .bind(ulo_http_hyper::Server::new(format!("127.0.0.1:{port}")))
        .listen()
        .await?;
    for bound in edge.addresses() {
        println!("listening on http://{}", bound.addr);
    }
    edge.serve(shutdown_signal()).await?;
    stock_handle.close(Signal::new("edge closed")).await?;
    pricing_handle.close(Signal::new("edge closed")).await?;
    let _ = tokio::join!(stock, pricing);
    Ok(())
}
