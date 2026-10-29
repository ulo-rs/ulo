use std::error::Error;
use std::sync::Arc;

use ulo::{AppBuilder, App, Connected, Runtime};
use ulo_rpc::{RpcClient, Server};
use ulo_rpc_kafka::Kafka;
use ulo_rpc_mqtt::Mqtt;
use ulo_rpc_nats::Nats;
use ulo_rpc_rabbitmq::RabbitMq;
use ulo_rpc_redis::Redis;
use ulo_rpc_tcp::Tcp;

/// Example 25's handlers, unchanged: which link carries them is decided by `bind` and `client`.
pub use ex_25_rpc_basics::{Divide, Journal, builder as handlers};

pub fn builder() -> AppBuilder {
    handlers()
}

/// Where a link URL names the link: `tcp://127.0.0.1:7000`, `nats://host:4222`, `redis://host:6379`,
/// `mqtt://host:1883`, `amqp://host:5672` or `kafka://host:9092`.
fn split(url: &str) -> Result<(&str, &str), Box<dyn Error + Send + Sync>> {
    url.split_once("://").ok_or_else(|| format!("`{url}` is not a link URL: write `scheme://address`").into())
}

fn unknown(scheme: &str) -> Box<dyn Error + Send + Sync> {
    format!("no link for the scheme `{scheme}`: expected tcp, nats, redis, mqtt, amqp or kafka").into()
}

/// Serves the app's handlers over the link `url` names.
pub fn bind(app: App<Connected>, url: &str) -> Result<App<Connected>, Box<dyn Error + Send + Sync>> {
    let (scheme, address) = split(url)?;
    Ok(match scheme {
        "tcp" => app.bind(Server::new(Tcp::new(address))),
        "nats" => app.bind(Server::new(Nats::url(url))),
        "redis" => app.bind(Server::new(Redis::url(url))),
        "mqtt" => app.bind(Server::new(Mqtt::url(url))),
        "amqp" => app.bind(Server::new(RabbitMq::url(url))),
        "kafka" => app.bind(Server::new(Kafka::brokers(address))),
        other => return Err(unknown(other)),
    })
}

/// A client of the same link, built on `runtime`.
pub fn client(url: &str, runtime: Arc<dyn Runtime>) -> Result<RpcClient, Box<dyn Error + Send + Sync>> {
    let (scheme, address) = split(url)?;
    Ok(match scheme {
        "tcp" => RpcClient::new(Tcp::new(address), runtime)?,
        "nats" => RpcClient::new(Nats::url(url), runtime)?,
        "redis" => RpcClient::new(Redis::url(url), runtime)?,
        "mqtt" => RpcClient::new(Mqtt::url(url), runtime)?,
        "amqp" => RpcClient::new(RabbitMq::url(url), runtime)?,
        "kafka" => RpcClient::new(Kafka::brokers(address), runtime)?,
        other => return Err(unknown(other)),
    })
}
