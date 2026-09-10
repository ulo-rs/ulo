# #32 — feat: add toni-seaorm integration crate

Merged 2026-04-13 into `master` from `feat/seaorm-integration`, commit [`9b57654`](https://github.com/ulo-rs/ulo/commit/9b5765451a0c39425a043e388ff500392d1d8c43).

## Summary

Provides `SeaOrmModule::for_root(url)` — a `DynamicModule` that registers a `DatabaseConnection` globally so any injectable can receive it as a constructor dependency.

```rust
#[module(imports: [SeaOrmModule::for_root(env!("DATABASE_URL"))])]
pub struct AppModule;

#[injectable(pub struct UserService {
    db: DatabaseConnection,
})]
impl UserService {
    pub async fn find_all(&self) -> Result<Vec<user::Model>, DbErr> {
        user::Entity::find().all(&self.db).await
    }
}
```

The database backend is selected via feature flags (`sqlx-postgres`, `sqlx-mysql`, `sqlx-sqlite`) and runtime (`runtime-tokio-rustls`, `runtime-tokio-native-tls`), matching sea-orm's own feature gating.

Connection pool is closed cleanly on `on_application_shutdown`.

## Test plan

- [ ] `cargo check -p toni-seaorm` with at least one backend feature enabled
- [ ] A service injecting `DatabaseConnection` resolves correctly at startup
- [ ] Pool closes without error on application shutdown
