//! `Tls::load` answers a rustls `ServerConfig` that completes a handshake with no runtime at all:
//! the two rustls connections below exchange their records through plain buffers, so a backend on
//! any runtime gets TLS by wrapping the same configuration in its own TLS crate.
//!
//! The certificate is `ulo-test-certs`' self-signed one for `localhost`, made when the tests run.

use std::io::{Read, Write};
use std::sync::Arc;

use ulo_net::rustls::pki_types::ServerName;
use ulo_net::rustls::{self, ClientConfig, ClientConnection, ServerConnection};
use ulo_net::{KeyCause, Tls, TlsError};
use ulo_test_certs::{Certified, localhost};

fn served() -> Tls {
    let cert = localhost();
    Tls::from_pem(cert.cert_pem(), cert.key_pem())
}

fn client(alpn: &[&[u8]]) -> ClientConnection {
    let roots = localhost().roots();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("the default versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
    ClientConnection::new(Arc::new(config), ServerName::try_from("localhost").expect("a DNS name")).expect("the client starts")
}

#[test]
fn a_loaded_configuration_completes_a_handshake_without_a_runtime() {
    let config = served().load(&[b"h2", b"http/1.1"]).expect("the pair loads");
    let mut server = ServerConnection::new(config).expect("the server starts");
    let mut client = client(&[b"h2"]);

    let mut wire = Vec::new();
    while client.is_handshaking() || server.is_handshaking() {
        wire.clear();
        client.write_tls(&mut wire).expect("the client writes its records");
        server.read_tls(&mut wire.as_slice()).expect("the server reads them");
        server.process_new_packets().expect("the server accepts the client's records");
        wire.clear();
        server.write_tls(&mut wire).expect("the server writes its records");
        client.read_tls(&mut wire.as_slice()).expect("the client reads them");
        client.process_new_packets().expect("the client accepts the server's certificate");
    }
    assert_eq!(server.alpn_protocol(), Some(b"h2".as_slice()), "the ALPN offered at load settles the protocol");

    client.writer().write_all(b"ping").expect("the client writes");
    wire.clear();
    client.write_tls(&mut wire).expect("the client writes its record");
    server.read_tls(&mut wire.as_slice()).expect("the server reads it");
    server.process_new_packets().expect("the server decrypts it");
    let mut read = [0u8; 4];
    server.reader().read_exact(&mut read).expect("the server reads the plaintext");
    assert_eq!(&read, b"ping");
}

#[test]
fn load_offers_the_protocols_it_is_given_in_order() {
    let config = served().load(&[b"h2", b"http/1.1"]).expect("the pair loads");
    assert_eq!(config.alpn_protocols, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
}

#[test]
fn a_key_that_is_not_the_certificates_is_refused() {
    let other = Certified::self_signed(&["localhost"]);
    let refused = Tls::from_pem(other.cert_pem(), localhost().key_pem()).load(&[]).expect_err("a key not matching the certificate is refused");
    assert!(
        matches!(refused, TlsError::PrivateKey { path: None, cause: KeyCause::DoesNotMatchCertificate }),
        "refused as a key that is not the certificate's, got {refused:?}"
    );
}

/// Valid base64 that is not a key: the PEM parses and no supported key type reads what it holds.
#[test]
fn a_pem_section_holding_no_key_is_refused_as_unrecognised() {
    let not_a_key = "-----BEGIN PRIVATE KEY-----\nAAAAAAAA\n-----END PRIVATE KEY-----\n";
    let refused = Tls::from_pem(localhost().cert_pem(), not_a_key).load(&[]).expect_err("a section holding no key is refused");
    assert!(
        matches!(refused, TlsError::PrivateKey { path: None, cause: KeyCause::Unrecognised }),
        "refused as bytes that are no supported key, got {refused:?}"
    );
}

#[test]
fn an_unrecognised_key_file_is_refused_with_its_path() {
    let dir = std::env::temp_dir().join(format!("ulo-net-tls-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch directory");
    let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
    std::fs::write(&cert, localhost().cert_pem()).expect("write certificate");
    std::fs::write(&key, "-----BEGIN PRIVATE KEY-----\nAAAAAAAA\n-----END PRIVATE KEY-----\n").expect("write key");
    let refused = Tls::from_pem_files(&cert, &key).load(&[]).expect_err("a file holding no key is refused");
    let _ = std::fs::remove_dir_all(&dir);
    match refused {
        TlsError::PrivateKey { path: Some(path), cause: KeyCause::Unrecognised } => assert_eq!(path, key),
        other => panic!("a key file holding no key was {other:?}"),
    }
}

mod given_configuration {
    use super::*;

    const HTTP: &[&[u8]] = &[b"h2", b"http/1.1"];

    fn config_with(alpn: &[&[u8]]) -> Arc<rustls::ServerConfig> {
        let cert = localhost();
        let loaded = Tls::from_pem(cert.cert_pem(), cert.key_pem()).load(&[]).expect("the pair loads");
        let mut config = rustls::ServerConfig::clone(&loaded);
        config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
        Arc::new(config)
    }

    fn protocols(config: &rustls::ServerConfig) -> Vec<&[u8]> {
        config.alpn_protocols.iter().map(Vec::as_slice).collect()
    }

    #[test]
    fn a_configuration_naming_no_protocol_gets_the_ones_the_server_needs() {
        let given = config_with(&[]);
        let loaded = Tls::from_config(Arc::clone(&given)).load(HTTP).expect("an empty list is the server's to set");
        assert_eq!(protocols(&loaded), HTTP);
        assert!(given.alpn_protocols.is_empty(), "the handed configuration is not modified");
    }

    #[test]
    fn a_configuration_naming_both_is_accepted() {
        let loaded = Tls::from_config(config_with(HTTP)).load(HTTP).expect("both are named");
        assert_eq!(protocols(&loaded), HTTP);
    }

    #[test]
    fn the_servers_order_wins_over_the_configurations() {
        let loaded = Tls::from_config(config_with(&[b"http/1.1", b"h2"])).load(HTTP).expect("both are named");
        assert_eq!(protocols(&loaded), HTTP);
    }

    #[test]
    fn a_further_protocol_is_dropped_from_the_servers_list() {
        let given = config_with(&[b"h3", b"h2", b"http/1.1"]);
        let loaded = Tls::from_config(Arc::clone(&given)).load(HTTP).expect("a superset is accepted");
        assert_eq!(protocols(&loaded), HTTP);
        assert_eq!(protocols(&given), [b"h3".as_slice(), b"h2", b"http/1.1"], "the handed configuration is not modified");
    }

    #[test]
    fn acme_tls_1_is_kept_after_the_transports_protocols() {
        let given = config_with(&[b"acme-tls/1", b"http/1.1", b"h2"]);
        let loaded = Tls::from_config(Arc::clone(&given)).load(HTTP).expect("http names acme-tls/1 beside its pair");
        assert_eq!(protocols(&loaded), [b"h2".as_slice(), b"http/1.1", b"acme-tls/1"]);
        assert_eq!(protocols(&given), [b"acme-tls/1".as_slice(), b"http/1.1", b"h2"], "the handed configuration is not modified");
    }

    #[test]
    fn acme_tls_1_is_kept_on_ws_and_grpc_and_h3_is_still_dropped() {
        let ws = Tls::from_config(config_with(&[b"h2", b"h3", b"http/1.1", b"acme-tls/1"])).load(&[b"http/1.1"]).expect("http/1.1 is named");
        assert_eq!(protocols(&ws), [b"http/1.1".as_slice(), b"acme-tls/1"]);
        let grpc = Tls::from_config(config_with(&[b"http/1.1", b"h2", b"h3", b"acme-tls/1"])).load(&[b"h2"]).expect("h2 is named");
        assert_eq!(protocols(&grpc), [b"h2".as_slice(), b"acme-tls/1"]);
    }

    #[test]
    fn acme_tls_1_is_not_added_when_the_configuration_does_not_name_it() {
        let loaded = Tls::from_config(config_with(&[b"h2", b"http/1.1"])).load(HTTP).expect("both are named");
        assert_eq!(protocols(&loaded), HTTP);
        let empty = Tls::from_config(config_with(&[])).load(HTTP).expect("an empty list is the server's to set");
        assert_eq!(protocols(&empty), HTTP);
    }

    /// The match is on the exact name, and a configuration naming it twice is offered it once.
    #[test]
    fn acme_tls_1_named_twice_is_offered_once_and_a_lookalike_is_dropped() {
        let loaded = Tls::from_config(config_with(&[b"acme-tls/1", b"h2", b"acme-tls/2", b"http/1.1", b"acme-tls/1"])).load(HTTP).expect("both are named");
        assert_eq!(protocols(&loaded), [b"h2".as_slice(), b"http/1.1", b"acme-tls/1"]);
        let lookalike = Tls::from_config(config_with(&[b"h2", b"acme-tls/2", b"http/1.1"])).load(HTTP).expect("both are named");
        assert_eq!(protocols(&lookalike), HTTP, "only `acme-tls/1` itself is kept");
    }

    #[test]
    fn a_configuration_already_equal_to_the_list_with_acme_is_used_as_given() {
        let given = config_with(&[b"h2", b"http/1.1", b"acme-tls/1"]);
        let loaded = Tls::from_config(Arc::clone(&given)).load(HTTP).expect("both are named");
        assert!(Arc::ptr_eq(&given, &loaded));
    }

    #[test]
    fn a_transport_needing_no_protocol_keeps_acme_tls_1_as_given() {
        let given = config_with(&[b"acme-tls/1"]);
        let loaded = Tls::from_config(Arc::clone(&given)).load(&[]).expect("nothing is required");
        assert!(Arc::ptr_eq(&given, &loaded));
    }

    fn refused(alpn: &[&[u8]], missing: &[&str]) {
        let Err(error) = Tls::from_config(config_with(alpn)).load(HTTP) else { panic!("{alpn:?} must be refused") };
        let TlsError::Alpn { required, given, missing: lacking } = &error else { panic!("{error:?}") };
        assert_eq!(required, &["h2", "http/1.1"]);
        assert_eq!(given.len(), alpn.len());
        assert_eq!(lacking, missing);
        // The list rendered whole: each name alone also appears in the required list the message
        // gives beside it.
        let text = error.to_string();
        assert!(text.contains(&format!("{missing:?}")), "{text}");
    }

    #[test]
    fn http_1_1_alone_would_switch_http_2_off_and_is_refused() {
        refused(&[b"http/1.1"], &["h2"]);
    }

    #[test]
    fn h2_alone_would_strand_http_1_1_clients_and_is_refused() {
        refused(&[b"h2"], &["http/1.1"]);
    }

    /// The configuration names no application protocol, so it chose none: the server's are
    /// supplied, in its order, and `acme-tls/1` stays when named.
    #[test]
    fn a_configuration_naming_none_of_the_transports_protocols_is_given_them() {
        let given = config_with(&[b"acme-tls/1"]);
        let loaded = Tls::from_config(Arc::clone(&given)).load(HTTP).expect("acme-tls/1 alone makes no choice");
        assert_eq!(protocols(&loaded), [b"h2".as_slice(), b"http/1.1", b"acme-tls/1"]);
        assert_eq!(protocols(&given), [b"acme-tls/1".as_slice()], "the handed configuration is not modified");

        // `h3` alone chose a protocol HTTP's server does not speak.
        refused(&[b"h3"], &["h2", "http/1.1"]);

        let empty = Tls::from_config(config_with(&[])).load(HTTP).expect("an empty list makes no choice");
        let only_acme_without_it = Tls::from_config(config_with(&[b"acme-tls/1"])).load(HTTP).expect("loads");
        assert_eq!(protocols(&empty), &protocols(&only_acme_without_it)[..2], "empty and acme-only agree on the transport's part");
    }

    #[test]
    fn acme_only_on_ws_and_grpc_is_given_the_transports_protocol_and_keeps_acme() {
        let ws = Tls::from_config(config_with(&[b"acme-tls/1"])).load(&[b"http/1.1"]).expect("no choice");
        assert_eq!(protocols(&ws), [b"http/1.1".as_slice(), b"acme-tls/1"]);
        let grpc = Tls::from_config(config_with(&[b"acme-tls/1"])).load(&[b"h2"]).expect("no choice");
        assert_eq!(protocols(&grpc), [b"h2".as_slice(), b"acme-tls/1"]);
    }

    #[test]
    fn an_acme_only_configuration_on_http_serves_both_protocols_and_the_validator() {
        let loaded = Tls::from_config(config_with(&[b"acme-tls/1"])).load(HTTP).expect("no choice");
        assert_eq!(handshake(Arc::clone(&loaded), &[b"h2", b"http/1.1"]).expect("h2 in common"), Some(b"h2".to_vec()));
        assert_eq!(handshake(Arc::clone(&loaded), &[b"http/1.1"]).expect("http/1.1 in common"), Some(b"http/1.1".to_vec()));
        assert_eq!(handshake(loaded, &[b"acme-tls/1"]).expect("acme-tls/1 in common"), Some(b"acme-tls/1".to_vec()));
    }

    /// A configuration naming part of the transport's protocols has chosen against the rest.
    #[test]
    fn a_subset_of_the_transports_protocols_is_still_refused_beside_other_names() {
        refused(&[b"acme-tls/1", b"http/1.1"], &["h2"]);
        refused(&[b"h3", b"h2"], &["http/1.1"]);
    }

    #[test]
    fn a_transport_needing_no_protocol_takes_the_configuration_as_given() {
        let given = config_with(&[b"http/1.1"]);
        let loaded = Tls::from_config(Arc::clone(&given)).load(&[]).expect("nothing is required");
        assert!(Arc::ptr_eq(&given, &loaded));
    }

    #[test]
    fn a_copy_keeps_what_the_configuration_set_besides_its_protocols() {
        let mut config = rustls::ServerConfig::clone(&config_with(&[b"http/1.1", b"h2"]));
        config.max_fragment_size = Some(1024);
        config.send_tls13_tickets = 7;
        let given = Arc::new(config);
        let loaded = Tls::from_config(Arc::clone(&given)).load(HTTP).expect("both are named");
        assert!(!Arc::ptr_eq(&given, &loaded), "the reordered protocols are a copy's");
        assert!(Arc::ptr_eq(&given.session_storage, &loaded.session_storage), "session resumption's store");
        assert!(Arc::ptr_eq(&given.ticketer, &loaded.ticketer), "the ticketer");
        assert!(Arc::ptr_eq(&given.cert_resolver, &loaded.cert_resolver), "the certificate resolver");
        assert_eq!((loaded.max_fragment_size, loaded.send_tls13_tickets), (Some(1024), 7));
        assert_eq!(protocols(&given), [b"http/1.1".as_slice(), b"h2"], "the handed configuration is not modified");
    }

    #[test]
    fn a_grpc_server_offers_h2_alone() {
        let loaded = Tls::from_config(config_with(&[b"http/1.1", b"h2", b"h3"])).load(&[b"h2"]).expect("h2 is named");
        assert_eq!(protocols(&loaded), [b"h2".as_slice()]);
        // `http/1.1` alone chose a protocol gRPC does not speak.
        assert!(matches!(Tls::from_config(config_with(&[b"http/1.1"])).load(&[b"h2"]), Err(TlsError::Alpn { .. })));
    }

    #[test]
    fn a_ws_server_offers_http_1_1_alone() {
        let loaded = Tls::from_config(config_with(&[b"h2", b"http/1.1"])).load(&[b"http/1.1"]).expect("http/1.1 is named");
        assert_eq!(protocols(&loaded), [b"http/1.1".as_slice()]);
    }

    /// Runs a handshake until the server refuses or both sides finish; the server's verdict.
    fn handshake(config: Arc<rustls::ServerConfig>, offered: &[&[u8]]) -> Result<Option<Vec<u8>>, rustls::Error> {
        let mut server = rustls::ServerConnection::new(config).expect("the server starts");
        let mut client = client(offered);
        let mut wire = Vec::new();
        while client.is_handshaking() || server.is_handshaking() {
            wire.clear();
            client.write_tls(&mut wire).expect("the client writes");
            server.read_tls(&mut wire.as_slice()).expect("the server reads");
            server.process_new_packets()?;
            wire.clear();
            server.write_tls(&mut wire).expect("the server writes");
            client.read_tls(&mut wire.as_slice()).expect("the client reads");
            let _ = client.process_new_packets();
        }
        Ok(server.alpn_protocol().map(<[u8]>::to_vec))
    }

    #[test]
    fn a_client_offering_only_a_protocol_the_transport_does_not_speak_gets_an_alert() {
        let loaded = Tls::from_config(config_with(&[b"h2", b"http/1.1"])).load(&[b"h2"]).expect("h2 is named");
        let error = handshake(Arc::clone(&loaded), &[b"http/1.1"]).expect_err("no protocol in common");
        assert!(matches!(error, rustls::Error::NoApplicationProtocol), "{error:?}");
        assert_eq!(handshake(loaded, &[b"h2", b"http/1.1"]).expect("h2 in common"), Some(b"h2".to_vec()));
    }

    #[test]
    fn a_client_offering_only_acme_tls_1_completes_the_handshake_when_the_configuration_names_it() {
        let loaded = Tls::from_config(config_with(&[b"h2", b"http/1.1", b"acme-tls/1"])).load(HTTP).expect("both are named");
        assert_eq!(handshake(Arc::clone(&loaded), &[b"acme-tls/1"]).expect("acme-tls/1 in common"), Some(b"acme-tls/1".to_vec()));
        assert_eq!(handshake(loaded, &[b"acme-tls/1", b"h2", b"http/1.1"]).expect("h2 in common"), Some(b"h2".to_vec()), "HTTP clients keep the transport's preference");
    }

    #[test]
    fn a_client_offering_only_acme_tls_1_gets_an_alert_when_the_configuration_does_not_name_it() {
        let loaded = Tls::from_config(config_with(&[b"h2", b"http/1.1"])).load(HTTP).expect("both are named");
        let error = handshake(loaded, &[b"acme-tls/1"]).expect_err("no protocol in common");
        assert!(matches!(error, rustls::Error::NoApplicationProtocol), "{error:?}");
    }

    #[test]
    fn a_client_offering_only_h3_gets_an_alert_though_the_configuration_names_it() {
        let loaded = Tls::from_config(config_with(&[b"h3", b"h2", b"http/1.1", b"acme-tls/1"])).load(HTTP).expect("both are named");
        let error = handshake(loaded, &[b"h3"]).expect_err("h3 is dropped");
        assert!(matches!(error, rustls::Error::NoApplicationProtocol), "{error:?}");
    }
}
