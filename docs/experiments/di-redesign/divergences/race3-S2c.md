# race 3, S2c: `ulo-db-mongodb`

## Points resolved

- **Which failures of the SRV lookup in `create` are permanent: none.** `DnsResolve` is retryable.
  A missing SRV record and a resolver that is briefly unreachable arrive as the same
  `ErrorKind::DnsResolve { message }`, and the message is the only thing that tells them apart, so
  the store does not read it. The loop under `connect_within` bounds the retry either way. A TXT
  record carrying a bad option arrives as `InvalidArgument` and is permanent with the rest of that
  kind.
- **The permanent set is wider than the design's two kinds.** `Authentication` and `InvalidArgument`
  as designed, plus `InvalidTlsConfig` and `IncompatibleServer` (no retry changes a client's TLS
  files or a server's wire version), and `ErrorKind::Command` with code 18 (`AuthenticationFailed`),
  for a server that answers the refusal as a command error and not in the handshake. PROBES_db.md
  §8.3 observed only the first. The code-18 arm is unprobed.
- **Where the permanent mapping applies.** One `classify` serves `create` (parse and
  `Client::with_options`) and `ping`.
- **Options override the URI.** `max_pool_size`, `min_pool_size` and `app_name` set in the options
  replace the same setting in the URI after `ClientOptions::parse`; unset, the URI's value stands.
  The design is silent.
- **`secrets` returns the password as written and, when different, percent-decoded.** The URI's
  userinfo is percent-encoded, and a driver error printing the password prints the decoded form.
  The password is read from the authority up to the first `/` or `?`, userinfo split at the last
  `@`, password after the first `:`. An empty password registers nothing. The whole URI is already
  registered by the module as a `Secret` leaf.
- **`validate` checks beyond scheme and a host:** an empty entry in the host list, and more than one
  host under `mongodb+srv://`, which the driver would refuse in `parse` as `InvalidArgument`. Every
  problem is at path `uri` and carries no part of the URI.
- **`close` is the driver's graceful `shutdown()`**, which waits for live cursors and sessions. The
  destroy hook's bound ends a wait on a leaked one. `immediate(true)` would skip the wait and risk
  server-side leaks.
- **`ping` is `{ping: 1}` on the `admin` database**, so no database named in the URI is touched; the
  server creates none (PROBES_db.md §8.3).
- **`create` is not wrapped in `spawn_with` by the store.** `ulo-db`'s factory runs `S::create`
  through `spawn_with` (D1's `connect.rs`), and the store's future reads no ambient context before
  its first await. `Client::with_options` runs inside that task.

## Divergences

- `Cargo.toml`: the skeleton's `tracing` dependency is removed; nothing in the crate uses it.
- No signature changed.

## Replaced on `master`, not carried over

Nothing of `master`'s implementation was read or carried over: the skeleton had emptied the crate.

## Requests

None.

## Not verified

- Nothing was compiled. API use was read against `mongodb-3.9.1` in the registry:
  `ClientOptions::parse` (action, awaited, takes `&str`), `ClientOptions`'s public `max_pool_size`,
  `min_pool_size` and `app_name`, `Client::with_options`, `Database::run_command`,
  `Client::shutdown`, `ErrorKind` variants, `CommandError::code`, `mongodb::bson::doc!`. That each
  action's future is `Send` is assumed from the driver's own multi-threaded use; the compiler
  decides.
- `ErrorKind::Command` code 18 from `ping` is not probed.
