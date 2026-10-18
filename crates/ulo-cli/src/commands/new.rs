use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use colored::Colorize;
use tokio::fs;

use super::{is_keyword, render};

/// The version every `ulo-*` dependency of a generated project is pinned to, exactly. The
/// framework's crates are released together, and a caret requirement on a pre-release or a `0.x`
/// would let a later release meet templates written for an earlier one.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The oldest compiler the framework's own crates build with; a store crate that needs a newer one
/// raises it in the project's manifest.
const BASE_RUST_VERSION: &str = "1.88";

#[derive(clap::Args)]
pub struct NewArgs {
    /// The project's name, which is also its directory
    project_name: String,

    /// The transport the project serves
    #[arg(long, value_enum, default_value_t = Transport::Http)]
    transport: Transport,

    /// A database layer, named `<module>-<backend>`: `sqlx-postgres`, `diesel-sqlite`, `mongodb`
    #[arg(long, value_enum)]
    db: Option<Db>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Transport {
    /// HTTP/1.1 and HTTP/2 on hyper, with the health probes
    Http,
    /// WebSocket gateways on the HTTP server's port, with the health probes
    Ws,
    /// gRPC from a `.proto` file, with `grpc.health.v1`
    Grpc,
    /// RPC over TCP
    RpcTcp,
    /// RPC over UDP
    RpcUdp,
    /// RPC over NATS
    RpcNats,
    /// RPC over Redis Pub/Sub
    RpcRedis,
    /// RPC over RabbitMQ
    RpcRabbitmq,
    /// RPC over MQTT 5
    RpcMqtt,
    /// RPC over Kafka
    RpcKafka,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Db {
    SqlxPostgres,
    SqlxMysql,
    SqlxSqlite,
    SeaormPostgres,
    SeaormMysql,
    SeaormSqlite,
    DieselPostgres,
    DieselMysql,
    DieselSqlite,
    Mongodb,
    Redis,
}

/// What differs between transports: the handler file, the server `main` binds, the crates and the
/// modules `AppModule` imports.
struct Layout {
    /// The module holding the transport's handlers, and the file it is read from.
    handler_module: &'static str,
    handler: &'static str,
    handler_type: &'static str,
    main_uses: Vec<String>,
    server: String,
    /// The `[server]` key naming where it listens, and the value the template starts with.
    key: &'static str,
    value: &'static str,
    app_uses: Vec<&'static str>,
    register: Vec<&'static str>,
    dependencies: Vec<String>,
    /// Manifest sections after `[dependencies]`.
    sections: &'static str,
    files: Vec<(&'static str, &'static str)>,
}

/// One RPC transport's link crate.
struct Link {
    krate: &'static str,
    import: &'static str,
    constructor: &'static str,
    key: &'static str,
    value: &'static str,
}

impl Transport {
    fn layout(self) -> Layout {
        match self {
            Transport::Http => Layout {
                handler_module: "controller",
                handler: include_str!("../templates/new/http/controller.rs"),
                handler_type: "GreetingController",
                main_uses: vec!["use ulo_http_hyper::Server;".into()],
                server: "Server::new(settings.server.address.clone())".into(),
                key: "address",
                value: "127.0.0.1:3000",
                app_uses: vec!["use ulo_health::HealthModule;", "use ulo_health_http::HealthHttp;"],
                register: vec!["m.import(HealthModule::new());", "m.import(HealthHttp::new());"],
                dependencies: lines(&[
                    r#"ulo-health = "=@@version@@""#,
                    r#"ulo-health-http = "=@@version@@""#,
                    r#"ulo-http = "=@@version@@""#,
                    r#"ulo-http-hyper = "=@@version@@""#,
                ]),
                sections: "",
                files: Vec::new(),
            },
            Transport::Ws => Layout {
                handler_module: "gateway",
                handler: include_str!("../templates/new/ws/gateway.rs"),
                handler_type: "GreetingGateway",
                main_uses: vec!["use ulo_http_hyper::Server;".into()],
                server: "Server::new(settings.server.address.clone())".into(),
                key: "address",
                value: "127.0.0.1:3000",
                app_uses: vec![
                    "use ulo_health::HealthModule;",
                    "use ulo_health_http::HealthHttp;",
                    "use ulo_ws::WsModule;",
                ],
                register: vec![
                    "m.import(HealthModule::new());",
                    "m.import(HealthHttp::new());",
                    "m.import(WsModule::for_root());",
                ],
                dependencies: lines(&[
                    r#"ulo-health = "=@@version@@""#,
                    r#"ulo-health-http = "=@@version@@""#,
                    r#"ulo-http-hyper = "=@@version@@""#,
                    r#"ulo-ws = "=@@version@@""#,
                ]),
                sections: "",
                files: Vec::new(),
            },
            Transport::Grpc => Layout {
                handler_module: "controller",
                handler: include_str!("../templates/new/grpc/controller.rs"),
                handler_type: "GreeterController",
                main_uses: vec!["use ulo_grpc::Server;".into()],
                server: "Server::new(settings.server.address.clone())".into(),
                key: "address",
                value: "127.0.0.1:50051",
                app_uses: vec![
                    "use ulo_grpc::GrpcHealth;",
                    "use ulo_health::HealthModule;",
                    "use ulo_health_grpc::HealthGrpc;",
                ],
                register: vec![
                    "m.import(HealthModule::new());",
                    "m.import(GrpcHealth::module());",
                    "m.import(HealthGrpc::new());",
                ],
                // What the code `ulo-build` writes names: prost's messages and tonic's types.
                dependencies: lines(&[
                    r#"prost = "0.14""#,
                    r#"tonic = { version = "0.14", default-features = false, features = ["channel", "codegen"] }"#,
                    r#"tonic-prost = "0.14""#,
                    r#"ulo-grpc = "=@@version@@""#,
                    r#"ulo-health = "=@@version@@""#,
                    r#"ulo-health-grpc = "=@@version@@""#,
                ]),
                sections: "\n[build-dependencies]\nulo-build = \"=@@version@@\"\n",
                files: vec![
                    ("build.rs", include_str!("../templates/new/grpc/build.rs")),
                    ("proto/greeter.proto", include_str!("../templates/new/grpc/greeter.proto")),
                ],
            },
            Transport::RpcTcp
            | Transport::RpcUdp
            | Transport::RpcNats
            | Transport::RpcRedis
            | Transport::RpcRabbitmq
            | Transport::RpcMqtt
            | Transport::RpcKafka => {
                let link = self.link();
                Layout {
                    handler_module: "controller",
                    handler: include_str!("../templates/new/rpc/controller.rs"),
                    handler_type: "GreetingController",
                    main_uses: vec!["use ulo_rpc::Server;".into(), link.import.into()],
                    server: format!("Server::new({}(settings.server.{}.clone()))", link.constructor, link.key),
                    key: link.key,
                    value: link.value,
                    app_uses: Vec::new(),
                    register: Vec::new(),
                    dependencies: vec![
                        r#"ulo-rpc = "=@@version@@""#.to_owned(),
                        format!(r#"{} = "=@@version@@""#, link.krate),
                    ],
                    sections: "",
                    files: Vec::new(),
                }
            }
        }
    }

    /// The link crate of an RPC transport. Only called for one.
    fn link(self) -> Link {
        match self {
            Transport::RpcUdp => Link {
                krate: "ulo-rpc-udp",
                import: "use ulo_rpc_udp::Udp;",
                constructor: "Udp::new",
                key: "address",
                value: "127.0.0.1:7001",
            },
            Transport::RpcNats => Link {
                krate: "ulo-rpc-nats",
                import: "use ulo_rpc_nats::Nats;",
                constructor: "Nats::url",
                key: "url",
                value: "nats://127.0.0.1:4222",
            },
            Transport::RpcRedis => Link {
                krate: "ulo-rpc-redis",
                import: "use ulo_rpc_redis::Redis;",
                constructor: "Redis::url",
                key: "url",
                value: "redis://127.0.0.1:6379",
            },
            Transport::RpcRabbitmq => Link {
                krate: "ulo-rpc-rabbitmq",
                import: "use ulo_rpc_rabbitmq::RabbitMq;",
                constructor: "RabbitMq::url",
                key: "url",
                value: "amqp://guest:guest@127.0.0.1:5672/%2f",
            },
            Transport::RpcMqtt => Link {
                krate: "ulo-rpc-mqtt",
                import: "use ulo_rpc_mqtt::Mqtt;",
                constructor: "Mqtt::url",
                key: "url",
                value: "mqtt://127.0.0.1:1883",
            },
            Transport::RpcKafka => Link {
                krate: "ulo-rpc-kafka",
                import: "use ulo_rpc_kafka::Kafka;",
                constructor: "Kafka::brokers",
                key: "brokers",
                value: "127.0.0.1:9092",
            },
            Transport::RpcTcp | Transport::Http | Transport::Ws | Transport::Grpc => Link {
                krate: "ulo-rpc-tcp",
                import: "use ulo_rpc_tcp::Tcp;",
                constructor: "Tcp::new",
                key: "address",
                value: "127.0.0.1:7000",
            },
        }
    }
}

/// What differs between databases: the module `AppModule` imports, the crates, the service
/// template, and the configuration the module reads.
struct Store {
    app_use: &'static str,
    import: &'static str,
    dependencies: &'static [&'static str],
    service: &'static str,
    /// What `@@pool@@` stands for in the service template, where it has one.
    pool: &'static str,
    /// The `[database]` key holding the URL, and the value the template starts with.
    key: &'static str,
    url: &'static str,
    /// A URL the wiring test can use without a server behind it.
    test_url: &'static str,
    rust_version: &'static str,
}

impl Db {
    fn store(self) -> Store {
        const SQLX_POSTGRES: &str = r#"ulo-db-sqlx = { version = "=@@version@@", features = ["postgres"] }"#;
        const SQLX_MYSQL: &str = r#"ulo-db-sqlx = { version = "=@@version@@", features = ["mysql"] }"#;
        const SQLX_SQLITE: &str = r#"ulo-db-sqlx = { version = "=@@version@@", features = ["sqlite"] }"#;
        const SEAORM_POSTGRES: &str = r#"ulo-db-seaorm = { version = "=@@version@@", features = ["postgres"] }"#;
        const SEAORM_MYSQL: &str = r#"ulo-db-seaorm = { version = "=@@version@@", features = ["mysql"] }"#;
        const SEAORM_SQLITE: &str = r#"ulo-db-seaorm = { version = "=@@version@@", features = ["sqlite"] }"#;
        const SEA_ORM: &str = r#"sea-orm = { version = "2.0", default-features = false }"#;
        const DIESEL_POSTGRES: &str = r#"ulo-db-diesel = { version = "=@@version@@", features = ["postgres"] }"#;
        const DIESEL_MYSQL: &str = r#"ulo-db-diesel = { version = "=@@version@@", features = ["mysql"] }"#;
        const DIESEL_SQLITE: &str = r#"ulo-db-diesel = { version = "=@@version@@", features = ["sqlite"] }"#;
        const POSTGRES_URL: &str = "postgres://postgres:postgres@localhost:5432/app";
        const MYSQL_URL: &str = "mysql://root:root@localhost:3306/app";
        const SQLX_SQLITE_URL: &str = "sqlite://app.db?mode=rwc";

        let sqlx_template = include_str!("../templates/new/db/sqlx.rs");
        let seaorm_template = include_str!("../templates/new/db/seaorm.rs");
        let diesel_template = include_str!("../templates/new/db/diesel.rs");

        match self {
            Db::SqlxPostgres => Store {
                app_use: "use ulo_db_sqlx::{Postgres, SqlxModule};",
                import: r#"m.import(SqlxModule::<Postgres>::from_config(&self.config, "database"));"#,
                dependencies: &[SQLX_POSTGRES],
                service: sqlx_template,
                pool: "PgPool",
                key: "url",
                url: POSTGRES_URL,
                test_url: POSTGRES_URL,
                rust_version: "1.94",
            },
            Db::SqlxMysql => Store {
                app_use: "use ulo_db_sqlx::{MySql, SqlxModule};",
                import: r#"m.import(SqlxModule::<MySql>::from_config(&self.config, "database"));"#,
                dependencies: &[SQLX_MYSQL],
                service: sqlx_template,
                pool: "MySqlPool",
                key: "url",
                url: MYSQL_URL,
                test_url: MYSQL_URL,
                rust_version: "1.94",
            },
            Db::SqlxSqlite => Store {
                app_use: "use ulo_db_sqlx::{Sqlite, SqlxModule};",
                import: r#"m.import(SqlxModule::<Sqlite>::from_config(&self.config, "database"));"#,
                dependencies: &[SQLX_SQLITE],
                service: sqlx_template,
                pool: "SqlitePool",
                key: "url",
                url: SQLX_SQLITE_URL,
                test_url: "sqlite::memory:",
                rust_version: "1.94",
            },
            Db::SeaormPostgres => Store {
                app_use: "use ulo_db_seaorm::SeaOrmModule;",
                import: r#"m.import(SeaOrmModule::from_config(&self.config, "database"));"#,
                dependencies: &[SEA_ORM, SEAORM_POSTGRES],
                service: seaorm_template,
                pool: "",
                key: "url",
                url: POSTGRES_URL,
                test_url: POSTGRES_URL,
                rust_version: "1.94",
            },
            Db::SeaormMysql => Store {
                app_use: "use ulo_db_seaorm::SeaOrmModule;",
                import: r#"m.import(SeaOrmModule::from_config(&self.config, "database"));"#,
                dependencies: &[SEA_ORM, SEAORM_MYSQL],
                service: seaorm_template,
                pool: "",
                key: "url",
                url: MYSQL_URL,
                test_url: MYSQL_URL,
                rust_version: "1.94",
            },
            Db::SeaormSqlite => Store {
                app_use: "use ulo_db_seaorm::SeaOrmModule;",
                import: r#"m.import(SeaOrmModule::from_config(&self.config, "database"));"#,
                dependencies: &[SEA_ORM, SEAORM_SQLITE],
                service: seaorm_template,
                pool: "",
                key: "url",
                url: SQLX_SQLITE_URL,
                test_url: "sqlite::memory:",
                rust_version: "1.94",
            },
            Db::DieselPostgres => Store {
                app_use: "use ulo_db_diesel::{DieselModule, Postgres};",
                import: r#"m.import(DieselModule::<Postgres>::from_config(&self.config, "database"));"#,
                dependencies: &[DIESEL_POSTGRES],
                service: diesel_template,
                pool: "PgPool",
                key: "url",
                url: POSTGRES_URL,
                test_url: POSTGRES_URL,
                rust_version: "1.89",
            },
            Db::DieselMysql => Store {
                app_use: "use ulo_db_diesel::{DieselModule, MySql};",
                import: r#"m.import(DieselModule::<MySql>::from_config(&self.config, "database"));"#,
                dependencies: &[DIESEL_MYSQL],
                service: diesel_template,
                pool: "MySqlPool",
                key: "url",
                url: MYSQL_URL,
                test_url: MYSQL_URL,
                rust_version: "1.89",
            },
            Db::DieselSqlite => Store {
                app_use: "use ulo_db_diesel::{DieselModule, Sqlite};",
                import: r#"m.import(DieselModule::<Sqlite>::from_config(&self.config, "database"));"#,
                dependencies: &[DIESEL_SQLITE],
                service: diesel_template,
                pool: "SqlitePool",
                key: "url",
                url: "app.db",
                test_url: ":memory:",
                rust_version: "1.89",
            },
            Db::Mongodb => Store {
                app_use: "use ulo_db_mongodb::MongoModule;",
                import: r#"m.import(MongoModule::from_config(&self.config, "database"));"#,
                dependencies: &[r#"mongodb = "3.9""#, r#"ulo-db-mongodb = "=@@version@@""#],
                service: include_str!("../templates/new/db/mongodb.rs"),
                pool: "",
                key: "uri",
                url: "mongodb://localhost:27017",
                test_url: "mongodb://localhost:27017",
                rust_version: BASE_RUST_VERSION,
            },
            Db::Redis => Store {
                app_use: "use ulo_db_redis::RedisModule;",
                import: r#"m.import(RedisModule::from_config(&self.config, "database"));"#,
                dependencies: &[
                    r#"redis = { version = "1", features = ["aio", "connection-manager", "tokio-comp"] }"#,
                    r#"ulo-db-redis = "=@@version@@""#,
                ],
                service: include_str!("../templates/new/db/redis.rs"),
                pool: "",
                key: "url",
                url: "redis://localhost:6379",
                test_url: "redis://localhost:6379",
                rust_version: BASE_RUST_VERSION,
            },
        }
    }
}

fn lines(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_owned()).collect()
}

/// A project, as the files it is written as.
struct Plan {
    /// Paths relative to the project's root, with their contents.
    files: Vec<(String, String)>,
    /// The names of the crates the project depends on, which its own name may not equal.
    dependencies: Vec<String>,
}

impl Plan {
    fn build(name: &str, transport: Transport, db: Option<Db>) -> Result<Plan> {
        let layout = transport.layout();
        let store = db.map(Db::store);
        let version = [("version", VERSION)];

        let mut dependencies: Vec<String> = lines(&[
            r#"serde = { version = "1", features = ["derive"] }"#,
            r#"tokio = { version = "1", features = ["macros", "rt-multi-thread"] }"#,
            r#"tracing = "0.1""#,
            r#"tracing-subscriber = { version = "0.3", features = ["env-filter"] }"#,
            r#"ulo = "=@@version@@""#,
            r#"ulo-config = "=@@version@@""#,
            r#"ulo-runtime-tokio = "=@@version@@""#,
        ]);
        dependencies.extend(layout.dependencies.iter().cloned());
        if let Some(store) = &store {
            dependencies.extend(store.dependencies.iter().map(|line| (*line).to_owned()));
        }
        dependencies.sort();
        let dependency_names: Vec<String> = dependencies
            .iter()
            .filter_map(|line| line.split_whitespace().next())
            .map(str::to_owned)
            .collect();
        let dependencies = render(&dependencies.join("\n"), &version)?;
        let sections = render(layout.sections, &version)?;

        let rust_version = store.as_ref().map_or(BASE_RUST_VERSION, |store| store.rust_version);
        let manifest = render(
            include_str!("../templates/new/Cargo.txt"),
            &[
                ("name", name),
                ("rust_version", rust_version),
                ("dependencies", dependencies.as_str()),
                ("sections", sections.as_str()),
            ],
        )?;

        let env_key = layout.key.to_uppercase();
        let mut config = render(
            include_str!("../templates/new/config.toml"),
            &[("key", layout.key), ("env_key", env_key.as_str()), ("value", layout.value)],
        )?;
        if let Some(store) = &store {
            config.push_str(&format!("\n[database]\n{} = \"{}\"\n", store.key, store.url));
        }

        let mut modules = vec!["mod app;".to_owned(), format!("mod {};", layout.handler_module), "mod greeter;".to_owned()];
        let mut main_uses = vec![
            "use app::{AppConfig, AppModule};".to_owned(),
            "use ulo::App;".to_owned(),
            "use ulo_config::{Config, Env, File};".to_owned(),
            "use ulo_runtime_tokio::{Tokio, shutdown_signal};".to_owned(),
        ];
        main_uses.extend(layout.main_uses.iter().cloned());
        main_uses.sort();
        if store.is_some() {
            modules.push("mod database;".to_owned());
        }
        modules.sort();
        let main_uses = main_uses.join("\n");
        let modules = modules.join("\n");
        let main = render(
            include_str!("../templates/new/common/main.rs"),
            &[("uses", main_uses.as_str()), ("modules", modules.as_str()), ("server", layout.server.as_str())],
        )?;

        let mut external: Vec<String> = [
            "use serde::Deserialize;",
            "use ulo::{Module, ModuleDef, ModuleIdentity};",
            "use ulo_config::{Config, ConfigModule};",
        ]
        .iter()
        .chain(layout.app_uses.iter())
        .map(|line| (*line).to_owned())
        .collect();
        let mut local = vec![
            format!("use crate::{}::{};", layout.handler_module, layout.handler_type),
            "use crate::greeter::Greeter;".to_owned(),
        ];
        let mut register = vec!["m.import(ConfigModule::<AppConfig>::new(&self.config));".to_owned()];
        register.extend(layout.register.iter().map(|line| (*line).to_owned()));
        let mut values = vec![format!("                    .set(\"server.{}\", \"{}\")", layout.key, layout.value)];
        if let Some(store) = &store {
            external.push(store.app_use.to_owned());
            local.push("use crate::database::Database;".to_owned());
            register.push(store.import.to_owned());
            register.push("m.provide::<Database>();".to_owned());
            values.push(format!("                    .set(\"database.{}\", \"{}\")", store.key, store.test_url));
        }
        register.push("m.provide::<Greeter>();".to_owned());
        register.push(format!("m.controller::<{}>();", layout.handler_type));
        external.sort();
        local.sort();
        let register: Vec<String> = register.iter().map(|line| format!("        {line}")).collect();
        let uses = format!("{}\n\n{}", external.join("\n"), local.join("\n"));
        let register = register.join("\n");
        let values = values.join("\n");
        let app = render(
            include_str!("../templates/new/common/app.rs"),
            &[
                ("uses", uses.as_str()),
                ("key", layout.key),
                ("register", register.as_str()),
                ("values", values.as_str()),
            ],
        )?;

        let mut files = vec![
            ("Cargo.toml".to_owned(), manifest),
            (".gitignore".to_owned(), include_str!("../templates/new/gitignore.txt").to_owned()),
            ("config/app.toml".to_owned(), config),
            ("src/main.rs".to_owned(), main),
            ("src/app.rs".to_owned(), app),
            ("src/greeter.rs".to_owned(), include_str!("../templates/new/common/greeter.rs").to_owned()),
            (format!("src/{}.rs", layout.handler_module), layout.handler.to_owned()),
        ];
        if let Some(store) = &store {
            files.push(("src/database.rs".to_owned(), render(store.service, &[("pool", store.pool)])?));
        }
        for (path, content) in &layout.files {
            files.push(((*path).to_owned(), render(content, &version)?));
        }
        Ok(Plan { files, dependencies: dependency_names })
    }
}

pub async fn execute(args: NewArgs) -> Result<()> {
    let plan = Plan::build(&args.project_name, args.transport, args.db)?;
    validate_project_name(&args.project_name, &plan.dependencies)?;

    let root = PathBuf::from(&args.project_name);
    // `create_dir` refuses an existing directory atomically, so a project is never written into
    // one that appeared after the check, and the cleanup below only ever removes what this made.
    fs::create_dir(&root)
        .await
        .with_context(|| format!("Failed to create directory '{}'", args.project_name))?;
    if let Err(error) = write_files(&root, &plan.files).await {
        let _ = fs::remove_dir_all(&root).await;
        return Err(error);
    }

    let transport = value_name(args.transport);
    let layers = match args.db {
        Some(db) => format!("transport {transport}, database {}", value_name(db)),
        None => format!("transport {transport}"),
    };
    println!(
        "\n{}",
        format!(
            "Created project '{name}' ({layers}).\n\nNext steps:\n  cd {name}\n  cargo test\n  cargo run\n\nThe addresses it serves on and connects to are in config/app.toml.",
            name = args.project_name,
        )
        .green()
    );
    Ok(())
}

/// The name a `ValueEnum` variant has on the command line.
fn value_name<T: ValueEnum>(value: T) -> String {
    value.to_possible_value().map(|value| value.get_name().to_owned()).unwrap_or_default()
}

fn validate_project_name(name: &str, dependencies: &[String]) -> Result<()> {
    if name.is_empty() {
        bail!("Project name cannot be empty");
    }
    if name.chars().any(|c| !c.is_ascii_alphanumeric() && c != '-' && c != '_') {
        bail!("Project name contains invalid characters. Use only alphanumerics, '-' or '_'");
    }
    if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        bail!("Project name must start with a letter");
    }
    if is_keyword(name) || is_keyword(&name.replace('-', "_")) {
        bail!("'{name}' is a Rust keyword and cannot name a package");
    }
    // Cargo refuses a package named for a crate it depends on, and a name that shadows the
    // standard library's.
    let crate_name = name.replace('-', "_");
    let taken = dependencies.iter().any(|dependency| dependency.replace('-', "_") == crate_name);
    if taken || ["std", "core", "alloc", "proc_macro", "test"].contains(&crate_name.as_str()) {
        bail!("'{name}' is the name of a crate the project depends on; choose another");
    }
    if Path::new(name).exists() {
        bail!("Directory '{name}' already exists");
    }
    Ok(())
}

async fn write_files(root: &Path, files: &[(String, String)]) -> Result<()> {
    for (path, content) in files {
        let full = root.join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)
                .await
                .with_context(|| format!("Failed to create directory {}", parent.display()))?;
        }
        fs::write(&full, content)
            .await
            .with_context(|| format!("Failed to write {}", full.display()))?;
    }
    Ok(())
}
