use app::app_module::AppModule;
use ulo::UloFactory;
use ulo_http_axum::AxumAdapter;

mod app;

#[tokio::main]
async fn main() {
    let mut app = UloFactory::new()
        .create_with(AppModule)
        .await.unwrap();

    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000))
        .unwrap();

    app.start().await.unwrap();
}
