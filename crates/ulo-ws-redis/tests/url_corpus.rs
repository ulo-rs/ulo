//! `prepare` refuses a URL exactly where `redis::Client::open` refuses it, over a corpus of the
//! URLs the redis client reads differently: TLS with and without `#insecure`, Unix sockets under
//! both schemes, databases in the path and in the query, passwords holding `@`, `%`, `#` and `?`,
//! IPv6 hosts, the `protocol` parameter, and URLs it does not read at all. No password in the
//! corpus reaches the adapter's settings or `prepare`'s refusal. Nothing connects.

use ulo_ws::BroadcastAdapter;
use ulo_ws_redis::Redis;

/// The password the credential-bearing URLs carry, in the userinfo or in the query.
const PASSWORD: &str = "corpus-secret";

const CORPUS: &[&str] = &[
    "redis://a:6379",
    "redis://a",
    "redis://a:6379/0",
    "redis://a/7",
    "redis://a//7",
    "redis://a:6379/-1",
    "redis://a:6379/x",
    "redis://a:6379/1/2",
    "redis://a/%31",
    "redis://a:6379/0?db=3",
    "rediss://a:6380",
    "rediss://a:6380/#insecure",
    "rediss://a:6380/2#insecure",
    "rediss://a/#verify",
    "redis://a/#insecure",
    "valkey://a",
    "valkeys://a/#insecure",
    "REDIS://A:6379",
    " redis://a",
    "redis+unix:///run/redis.sock",
    "unix:///run/redis.sock",
    "redis+unix:///run/redis.sock?db=2&pass=corpus-secret",
    "redis+unix:///run/redis.sock?db=x&pass=corpus-secret",
    "redis+unix://",
    "unix:relative.sock",
    "redis://user:corpus-secret@a:6379",
    "redis://:corpus%40secret@a:6379",
    "redis://:corpus-secret@with@a",
    "redis://:corpus%25secret@a",
    "redis://:corpus%23secret@a",
    "redis://:corpus#secret@a",
    "redis://:corpus%3Fsecret@a",
    "redis://:corpus?secret@a",
    "redis://%ff:corpus-secret@a",
    "redis://:%ff@a",
    "redis://[::1]:6379",
    "redis://[::1]",
    "redis://::1:6379",
    "redis://a:6379/?protocol=resp3",
    "redis://a/?protocol=3",
    "redis://a/?protocol=resp2",
    "redis://a/?protocol=resp9",
    "redis://a/?protocol=",
    "redis+unix:///run/redis.sock?protocol=resp3",
    "redis+unix:///run/redis.sock?protocol=resp4",
    "redis://0.0.0.0:6379",
    "redis://a:99999",
    "redis://a:0",
    "",
    "a:6379",
    "localhost:6379",
    "http://a:6379",
    "redis://",
];

/// Planted break: `prepare` refusing a URL the client reads, as one parsing the URL itself
/// refuses a host or a path the client accepts.
#[tokio::test]
async fn prepare_refuses_a_url_exactly_where_the_redis_client_does() {
    let mut disagreements = Vec::new();
    let (mut accepted, mut refused) = (0, 0);
    for url in CORPUS {
        let client = redis::Client::open(*url).map(drop).map_err(|error| error.to_string());
        let adapter = Redis::url(*url);
        let prepared = adapter.prepare().map_err(|error| error.to_string());
        if client.is_ok() != prepared.is_ok() {
            disagreements.push(format!("{url:?}: the client {client:?}, `prepare` {prepared:?}"));
        }
        if client.is_ok() { accepted += 1 } else { refused += 1 }
        let settings = format!("{:?}", adapter.settings());
        assert!(!settings.contains(PASSWORD) && !settings.contains("corpus%"), "{url:?} reached the settings: {settings}");
        if let Err(text) = &prepared {
            assert!(!text.contains(PASSWORD), "{url:?} reached the refusal: {text}");
        }
    }
    assert!(disagreements.is_empty(), "`prepare` and the redis client disagree:\n{}", disagreements.join("\n"));
    // A corpus the client reads whole, or refuses whole, shows nothing about the refusal.
    assert!(accepted >= 10 && refused >= 10, "{accepted} URLs accepted and {refused} refused");
}
