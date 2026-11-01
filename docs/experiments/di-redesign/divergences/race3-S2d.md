# Divergences: race 3, S2d (`ulo-db-redis`)

Files written: `crates/ulo-db-redis/src/{lib,options,store}.rs`. `Cargo.toml` is unchanged: it already
holds the workspace `redis` with `connection-manager` and the `tls` feature. No cargo was run; the
redis 1.7.1 items used were read in the registry (`src/aio/connection_manager.rs`, `src/client.rs`,
`src/connection.rs`, `src/errors/redis_error.rs`, `src/aio/runtime.rs`).

## Divergences

1. **S3 does not hold as ruled; the guard is built without the call.** The ruling is `prefer_tokio()`
   once per process under an `OnceLock`, a refusal read for the current preference, `Permanent` for
   a non-tokio one. In redis 1.7.1 `prefer_tokio()` exists only under `cfg(all(feature = "smol-comp",
   feature = "tokio-comp"))` (`src/aio/runtime.rs:93-105`, re-exported at `src/aio/mod.rs:172-175`),
   this crate enables `tokio-comp` alone (and must not enable `smol-comp`), and the preference
   (`CHOSEN_RUNTIME`, `:76`) has no reader. So: `prefer_tokio_once` wraps a `TOKIO_PREFERENCE:
   OnceLock<Result<(), String>>` whose initialiser records `Ok(())` and a `debug` event, makes no
   redis call, and its `Err` arm (a `Permanent`) is unreachable until a designer answer gives it
   a source. Premise "`ulo-db-redis` can set the preference": does not hold for the reason above.
   Consequence: a dependency that calls `prefer_smol()` is not detected and no `warn` is emitted,
   because a warn naming a conflict would fire on every start with no conflict in evidence. The
   design's `warn` branch needs a failed `prefer_tokio()` to trigger it, and none can occur. Stated
   in the crate docs. Options for the designer: accept this; or add `smol-comp` to the redis entry
   (refused for now); or drop S3 from the design.
2. **Where the guard is called.** The plan has "the first pool factory" call it. `Store` has no hook
   there, and the factory runs `Store::create` on the app's runtime after the name check, so
   `create` calls `prefer_tokio_once()` first. Every `create` retry reads the recorded outcome.
3. **`Store::secrets` is implemented** (the skeleton's D1 addition): the password component as
   parsed by `redis::Client::open` (percent-decoded), empty when absent or unparseable. The whole
   URL is registered by the section read, as D1/S-skel state.
4. **`validate` is stricter than the skeleton's comment.** Besides the scheme and `Client::open`, a
   zero `connection_timeout_secs` or `response_timeout_secs` is a problem at its own key, and
   `rediss://` without this crate's `tls` feature is a problem at `url` naming the feature (without
   it redis-rs fails `Client::open` with "can't connect with TLS", `connection.rs:571-575`).
   Messages are fixed text: neither the URL nor redis-rs's error text is written. `valkey://` and
   the unix-socket schemes, which redis-rs accepts, are refused by the scheme check.

## Points this area resolves

- **The bound of one `create` attempt inside D1's loop.** `ConnectionManagerConfig::
  set_connection_timeout(Some(t))` with `t` = `connection_timeout_secs` or 5 s (redis-rs defaults
  to 1 s, short for TLS to a remote host). The response timeout is set only when the options
  give one, otherwise redis-rs's 500 ms stays, which also bounds the health ping. The handshake
  after the TCP/TLS connect is bounded by the response timeout, not the connection timeout; the
  crate has no tokio dependency to wrap the call in a timer, and `connect_within` bounds the loop.
- **Classification.** `ErrorKind::AuthenticationFailed` and `InvalidClientConfig` are wrapped in
  `Permanent`, in `create` and `ping` alike; everything else, `Io` and timeouts included, is bare.
- **Close.** `ConnectionManager` has no close; `close` drops the handle it is given. A consumer's
  clone keeps the connection alive, which is documented on `close`.

## Not verified (no compile)

- `ConnectionManager::new_with_config(...)`'s future being `Send` (the library spawns on tokio, and
  the skeleton's `BoxFuture` requires it); read as `Send` but not compiled.
- `redis::Client::open(&str)` borrowing `url.expose().as_str()` inside the async block: the `Secret`
  is moved in, the borrow is within one expression.
- `Permanent::new(String)` and `Permanent::new(RedisError)` into `BoxError` (`Error + Send + Sync`).
- Whether redis-rs reports a wrong password at connect as `AuthenticationFailed` for every server
  version: PROBES_db.md §5 observed it on one.

## Requests

None.
