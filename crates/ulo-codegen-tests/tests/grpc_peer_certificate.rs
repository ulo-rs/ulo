//! A gRPC guard reads the client certificate its call's TLS session verified through
//! `GrpcCx::tls`, as an HTTP guard reads it through `HttpCx::conn`: the leaf a certified client
//! presented, `None` for a client that presented none, and never a certificate the call's
//! metadata names, which any client can write.

mod support;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tonic::metadata::MetadataValue;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};
use ulo::{BoxError, Dep, Guard, Module, ModuleDef, ModuleIdentity, controller, handlers, injectable};
use ulo_codegen_tests::pb::counter_client::CounterClient;
use ulo_codegen_tests::pb::{self, AddRequest, Sum};
use ulo_grpc::{Grpc, GrpcCx, Message};
use ulo_net::Tls;
use ulo_net::rustls::pki_types::pem::PemObject;
use ulo_net::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use ulo_net::rustls::server::WebPkiClientVerifier;
use ulo_net::rustls::{self, ServerConfig};
use ulo_test_certs::{Ca, Certified};

use support::{Running, within};

/// The metadata keys a proxy terminating TLS commonly writes the client's certificate into,
/// lowercase as HTTP/2 carries every key; each is also sent as a `-bin` key.
const FORGED: [&str; 7] =
    ["x-client-cert", "x-ssl-client-cert", "ssl-client-cert", "x-forwarded-client-cert", "x-amzn-mtls-clientcert", "x-ssl-cert", "x-client-certificate"];

/// What [`ReadsLeaf`] read off each call it admitted: the verified leaf, or `None`.
#[derive(Clone, Default)]
struct Leaves(Arc<Mutex<Vec<Option<Vec<u8>>>>>);

impl Leaves {
    fn take(&self) -> Vec<Option<Vec<u8>>> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// Records the leaf the call's TLS session verified, and admits the call.
#[injectable]
struct ReadsLeaf {
    seen: Dep<Leaves>,
}

impl Guard<Grpc> for ReadsLeaf {
    async fn can_activate(&self, cx: &GrpcCx) -> Result<bool, BoxError> {
        let leaf = cx.tls().and_then(|tls| tls.peer_certificates.as_ref()).and_then(|chain| chain.leaf()).map(<[u8]>::to_vec);
        self.seen.0.lock().unwrap_or_else(PoisonError::into_inner).push(leaf);
        Ok(true)
    }
}

#[controller]
struct Counter;

#[handlers]
#[guards(grpc = ReadsLeaf)]
impl Counter {
    #[ulo_grpc::method(pb::counter::Add)]
    fn add(&self, req: Message<AddRequest>) -> Sum {
        Sum { sum: req.0.a + req.0.b }
    }
}

struct Root(Leaves);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.provide::<ReadsLeaf>();
        m.controller::<Counter>();
    }
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A server for `server` that verifies client certificates `ca` issued and admits a client
/// presenting none, so the guard is what reads the difference.
fn verifying(server: &Certified, ca: &Ca) -> ulo_grpc::Server {
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(ca.roots()), provider())
        .allow_unauthenticated()
        .build()
        .expect("the verifier builds");
    let chain = CertificateDer::pem_slice_iter(server.cert_pem().as_bytes()).collect::<Result<Vec<_>, _>>().expect("the certificate parses");
    let key = PrivateKeyDer::from_pem_slice(server.key_pem().as_bytes()).expect("the key parses");
    let config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .expect("the default versions")
        .with_client_cert_verifier(verifier)
        .with_single_cert(chain, key)
        .expect("the pair is consistent");
    ulo_grpc::Server::new("127.0.0.1:0").reflection(false).tls(Tls::from_config(Arc::new(config)))
}

/// A channel to `app` trusting `ca`, presenting `client` when given.
async fn channel(app: &Running, ca: &Ca, client: Option<&Certified>) -> Channel {
    let mut tls = ClientTlsConfig::new().ca_certificate(Certificate::from_pem(ca.cert_pem())).domain_name("localhost");
    if let Some(client) = client {
        tls = tls.identity(Identity::from_pem(client.cert_pem(), client.key_pem()));
    }
    let endpoint = Endpoint::from_shared(format!("https://{}", app.addr)).expect("an endpoint");
    let endpoint = endpoint.tls_config(tls).expect("a TLS configuration").connect_timeout(Duration::from_secs(3));
    within("the TLS connection", endpoint.connect()).await.unwrap_or_else(|error| panic!("the client did not connect: {error}"))
}

/// One `Add` call over `channel`, carrying every [`FORGED`] key set to `forged` when it is given.
async fn add(channel: Channel, forged: Option<&str>) -> Sum {
    let mut request = tonic::Request::new(AddRequest { a: 2, b: 3 });
    if let Some(forged) = forged {
        for key in FORGED {
            request.metadata_mut().insert(key, MetadataValue::try_from(forged).expect("an ASCII value"));
            request.metadata_mut().insert_bin(format!("{key}-bin").parse::<tonic::metadata::BinaryMetadataKey>().expect("a binary key"), tonic::metadata::BinaryMetadataValue::from_bytes(forged.as_bytes()));
        }
    }
    within("the call", CounterClient::new(channel).add(request)).await.unwrap_or_else(|status| panic!("Add failed: {status:?}")).into_inner()
}

#[tokio::test]
async fn a_guard_reads_the_leaf_the_tls_session_verified_and_none_without_one() {
    let (ca, leaves) = (Ca::new(), Leaves::default());
    let app = Running::start(Root(leaves.clone()), verifying(&ca.issue(&["localhost", "127.0.0.1"]), &ca)).await;
    let client = ca.issue(&["client"]);

    assert_eq!(add(channel(&app, &ca, Some(&client)).await, None).await, Sum { sum: 5 });
    assert_eq!(leaves.take(), [Some(client.cert_der().to_vec())], "the guard reads the certified client's leaf");

    assert_eq!(add(channel(&app, &ca, None).await, None).await, Sum { sum: 5 });
    assert_eq!(leaves.take(), [None], "a client presenting no certificate, after a certified one");
    app.stop().await;
}

#[tokio::test]
async fn a_certificate_named_in_the_metadata_is_never_read_as_one() {
    let (ca, leaves) = (Ca::new(), Leaves::default());
    let app = Running::start(Root(leaves.clone()), verifying(&ca.issue(&["localhost", "127.0.0.1"]), &ca)).await;
    let client = ca.issue(&["client"]);
    let forged = client.cert_pem().replace('\n', "");

    assert_eq!(add(channel(&app, &ca, None).await, Some(&forged)).await, Sum { sum: 5 });
    assert_eq!(leaves.take(), [None], "a TLS client presenting no certificate, its metadata naming one");
    app.stop().await;

    let plain = Running::start(Root(leaves.clone()), ulo_grpc::Server::new("127.0.0.1:0").reflection(false)).await;
    let endpoint = Endpoint::from_shared(format!("http://{}", plain.addr)).expect("an endpoint");
    let channel = within("the plaintext connection", endpoint.connect()).await.unwrap_or_else(|error| panic!("the client did not connect: {error}"));
    assert_eq!(add(channel, Some(&forged)).await, Sum { sum: 5 });
    assert_eq!(leaves.take(), [None], "a plaintext client, its metadata naming a certificate");
    plain.stop().await;
}

/// The leaf belongs to the connection that carried the call: two calls on a certified connection
/// read its leaf, a call on another, uncertified connection opened meanwhile reads `None`, and the
/// certified connection still reads its leaf after it.
#[tokio::test]
async fn the_leaf_is_the_call_s_own_connection_s() {
    let (ca, leaves) = (Ca::new(), Leaves::default());
    let app = Running::start(Root(leaves.clone()), verifying(&ca.issue(&["localhost", "127.0.0.1"]), &ca)).await;
    let client = ca.issue(&["client"]);
    let certified = channel(&app, &ca, Some(&client)).await;
    let plain = channel(&app, &ca, None).await;
    let leaf = Some(client.cert_der().to_vec());

    assert_eq!(add(certified.clone(), None).await, Sum { sum: 5 });
    assert_eq!(add(certified.clone(), None).await, Sum { sum: 5 });
    assert_eq!(add(plain.clone(), None).await, Sum { sum: 5 });
    assert_eq!(add(certified, None).await, Sum { sum: 5 });
    assert_eq!(add(plain, None).await, Sum { sum: 5 });
    assert_eq!(leaves.take(), [leaf.clone(), leaf.clone(), None, leaf, None]);
    app.stop().await;
}
