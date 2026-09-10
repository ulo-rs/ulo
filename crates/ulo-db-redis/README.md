# ulo-db-redis

Redis integration for the [Ulo framework](https://github.com/ulo-rs/ulo).

Registers a `redis::aio::ConnectionManager` in Ulo's DI container so any injectable can declare it as a dependency. The connection manager automatically reconnects on failure and multiplexes commands over a single connection.

## Installation

```toml
[dependencies]
ulo-db-redis = "0.1"
```

## Setup

Import `RedisModule::for_root` once in your root module. `ConnectionManager` becomes available to every module in the application without further imports.

```rust
use ulo_db_redis::RedisModule;

#[module(imports: [RedisModule::for_root(env!("REDIS_URL"))])]
pub struct AppModule;
```

## Injecting the connection

Declare `ConnectionManager` as a field in any injectable. Use `AsyncCommands` to run commands:

```rust
use ulo_db_redis::{AsyncCommands, ConnectionManager, RedisResult};

#[injectable]
pub struct CacheService {
    #[inject]
    redis: ConnectionManager,
}

impl CacheService {
    pub async fn set(&self, key: &str, value: &str, ttl_secs: u64) -> RedisResult<()> {
        let mut conn = self.redis.clone();
        conn.set_ex(key, value, ttl_secs).await
    }

    pub async fn get(&self, key: &str) -> RedisResult<Option<String>> {
        let mut conn = self.redis.clone();
        conn.get(key).await
    }
}
```

`ConnectionManager` is `Clone` — each clone shares the same underlying multiplexed connection to Redis.

## License

MIT
