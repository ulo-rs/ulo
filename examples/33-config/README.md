# 33 config

Layered configuration. `ulo_config::Config` merges defaults, `config/app.toml`, `config/.env`, the environment and values set in code; each source overrides the ones before it. `ConfigModule<AppConfig>` registers the whole tree as one binding, and every problem in it is reported by `wire()` with its path and the file and line that supplied the value.

```
cargo test -p ex-33-config
PORT=7000 APP_GREETING=hi cargo run -p ex-33-config
curl http://127.0.0.1:7000/settings
```

`/settings` answers the effective values. The database URL is a `Secret`: its `Debug` is `Secret([redacted])`, and a report that names the URL's path never quotes the value. `.deny_unread()` refuses `connect` for a key no section reads, such as a misspelled `APP_HTTP__PROT`.

Read first:

- `src/lib.rs`: the five sources in the order they layer, and `AppConfig`.
- `src/main.rs`: where the process environment (`Env::process()`) and the explicit values enter.
- `tests/layers.rs`: one field per winning layer, a bad file whose every problem is reported, a refused secret among them, and `deny_unread`.
- `tests/binary.rs`: runs the compiled binary with `PORT` set to a free port and reads `/settings` back.

`ulo new` writes a project that loads its configuration this way (`crates/ulo-cli/src/templates/new`).

Next: [34 health](../34-health).
