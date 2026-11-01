@@uses@@

#[derive(Clone, Debug, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ServerConfig {
    pub @@key@@: String,
}

/// The root module. It holds the `Config` because modules that read their own section, as the
/// database module does, take it at registration, before anything is built.
pub struct AppModule {
    config: Config,
}

impl AppModule {
    pub fn new(config: Config) -> Self {
        AppModule { config }
    }
}

impl Module for AppModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
@@register@@
    }
}

#[cfg(test)]
mod tests {
    use ulo::TestApp;
    use ulo_config::{Config, Values};
    use ulo_runtime_tokio::Tokio;

    use super::AppModule;

    #[tokio::test]
    async fn the_application_wires() {
        let config = Config::builder()
            .source(
                Values::new()
@@values@@
            )
            .load();

        TestApp::of(AppModule::new(config))
            .runtime(Tokio::current())
            .wire()
            .expect("the application wires");
    }
}
