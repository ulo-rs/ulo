# 20 http-tls

HTTPS from a PEM certificate chain and key; `ulo-net` loads them into a rustls `ServerConfig` when the
app listens, offering `h2` and `http/1.1`.

```
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj /CN=localhost \
  -addext subjectAltName=DNS:localhost,IP:127.0.0.1 -keyout key.pem -out cert.pem
TLS_CERT=cert.pem TLS_KEY=key.pem cargo run -p ex-20-http-tls
curl --cacert cert.pem https://localhost:3000/
```

The server listens on `PORT` (default 3000). The tests generate their certificates with `ulo-test-certs`;
no key is committed.

Read first:

- `src/lib.rs`: `server` is `Server::new(..).tls(Tls::from_pem_files(..))`.
- `tests/tls.rs`: the trusted handshake, ALPN, plaintext to the TLS port, an untrusted certificate, and a
  bad certificate failing `listen`.

Previous: [19 http-uploads](../19-http-uploads). Next: [21 http-in-axum](../21-http-in-axum).
