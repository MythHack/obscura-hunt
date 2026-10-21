use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};

use obscura_net::{CookieJar, ObscuraHttpClient};
use rcgen::{BasicConstraints, CertificateParams, CustomExtension, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use url::Url;

// The test's child is the real public native client, with its own process-level
// CA configuration. The fixture never changes the test runner's environment.
#[tokio::test]
async fn native_aia_preserves_verification_and_never_replays_an_accepted_post() {
    if let Ok(base) = std::env::var("OBSCURA_AIA_FIXTURE") {
        let fake = std::env::var("OBSCURA_AIA_FAKE_FIXTURE").unwrap();
        let client = ObscuraHttpClient::with_full_options(Arc::new(CookieJar::new()), None, true);
        let url = |path: &str| Url::parse(&format!("{base}{path}")).unwrap();
        let response = client.fetch_with_method(reqwest::Method::POST, &url("/ok"), Some(vec![0, 255, 128, 195, 40]), None).await.unwrap();
        assert_eq!(response.body, b"verified");
        assert_eq!(client.fetch(&url("/cached")).await.unwrap().body, b"verified");
        let wrong_host = Url::parse(&format!("{}/wrong-host", base.replace("localhost", "127.0.0.1"))).unwrap();
        assert!(client.fetch(&wrong_host).await.is_err(), "recovered chain must still verify hostname");
        assert!(client.fetch(&Url::parse(&fake).unwrap()).await.is_err(), "same-name fake issuer must not verify");
        assert!(client.fetch_with_method(reqwest::Method::POST, &url("/drop"), Some(b"once".to_vec()), None).await.is_err());
        #[cfg(feature = "stealth")]
        {
            let client = obscura_net::StealthHttpClient::with_proxy(Arc::new(CookieJar::new()), None, true);
            let response = client.fetch_with_method("POST", &url("/ok"), Some(vec![0, 255, 128, 195, 40]), None, obscura_net::ResourceRequest::navigation()).await.unwrap();
            assert_eq!(response.body, b"verified");
            assert_eq!(client.fetch(&url("/cached")).await.unwrap().body, b"verified");
            assert!(client.fetch(&wrong_host).await.is_err());
            assert!(client.fetch(&Url::parse(&fake).unwrap()).await.is_err());
            assert!(client.fetch_with_method("POST", &url("/drop"), Some(b"once".to_vec()), None, obscura_net::ResourceRequest::navigation()).await.is_err());
        }
        return;
    }

    let files = tempfile::tempdir().unwrap();
    let root_key = KeyPair::generate().unwrap();
    let mut root_params = ca_params("AIA fixture root");
    root_params.key_usages.push(KeyUsagePurpose::CrlSign);
    let root = root_params.self_signed(&root_key).unwrap();
    let root_issuer = Issuer::new(root_params, root_key);
    let intermediate_key = KeyPair::generate().unwrap();
    let intermediate_params = ca_params("AIA fixture intermediate");
    let intermediate = intermediate_params.signed_by(&intermediate_key, &root_issuer).unwrap();
    let intermediate_issuer = Issuer::new(intermediate_params.clone(), intermediate_key);
    // Same subject and valid CA constraints, but a different signing key.
    let fake_key = KeyPair::generate().unwrap();
    let fake_intermediate = intermediate_params.signed_by(&fake_key, &root_issuer).unwrap();
    let roots = files.path().join("root.pem");
    std::fs::write(&roots, root.pem()).unwrap();

    let material = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let material_port = material.local_addr().unwrap().port();
    let aia_hits = Arc::new(AtomicUsize::new(0));
    let fake_hits = Arc::new(AtomicUsize::new(0));
    let hits = aia_hits.clone();
    let bad_hits = fake_hits.clone();
    let issuer_der = intermediate.der().to_vec();
    let fake_der = fake_intermediate.der().to_vec();
    let material_task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = material.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                let mut chunk = [0; 4096];
                let size = socket.read(&mut chunk).await.unwrap();
                if size == 0 { break; }
                request.extend_from_slice(&chunk[..size]);
            }
            let bad = request.starts_with(b"GET /fake ");
            if bad { bad_hits.fetch_add(1, Ordering::Relaxed); } else { hits.fetch_add(1, Ordering::Relaxed); }
            let body = if bad { &fake_der } else { &issuer_der };
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(body).await.unwrap();
        }
    });
    let records = Arc::new(Mutex::new(Vec::new()));
    let (good_port, good_task) = tls_fixture(&intermediate_issuer, &format!("http://127.0.0.1:{material_port}/issuer"), records.clone()).await;
    let (fake_port, fake_task) = tls_fixture(&intermediate_issuer, &format!("http://127.0.0.1:{material_port}/fake"), records.clone()).await;
    let output = tokio::time::timeout(std::time::Duration::from_secs(60),
        tokio::process::Command::new(std::env::current_exe().unwrap())
            .kill_on_drop(true)
            .args(["--exact", "native_aia_preserves_verification_and_never_replays_an_accepted_post", "--nocapture"])
            .env("OBSCURA_AIA_FIXTURE", format!("https://localhost:{good_port}"))
            .env("OBSCURA_AIA_FAKE_FIXTURE", format!("https://localhost:{fake_port}/fake"))
            .env("SSL_CERT_FILE", &roots).env_remove("SSL_CERT_DIR")
            .env_remove("HTTP_PROXY").env_remove("HTTPS_PROXY").env_remove("ALL_PROXY")
            .env_remove("http_proxy").env_remove("https_proxy").env_remove("all_proxy")
            .output()).await.expect("native TLS consumer exceeded hard fixture deadline").unwrap();
    material_task.abort(); good_task.abort(); fake_task.abort();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let transports = if cfg!(feature = "stealth") { 2 } else { 1 };
    let records = records.lock().unwrap();
    assert_eq!(records.len(), transports * 3, "only verified ok/cache/drop requests may reach HTTP: {records:?}");
    assert_eq!(records.iter().filter(|request| request.starts_with(b"POST /drop ")).count(), transports);
    assert_eq!(records.iter().filter(|request| request.starts_with(b"POST /ok ") && request.ends_with(&[0, 255, 128, 195, 40])).count(), transports);
    assert!(records.iter().all(|request| !request.starts_with(b"HEAD ")));
    assert_eq!(aia_hits.load(Ordering::Relaxed), transports, "second connection must use bounded issuer material cache");
    assert_eq!(fake_hits.load(Ordering::Relaxed), transports);
}

fn ca_params(name: &str) -> CertificateParams {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.distinguished_name.push(DnType::CommonName, name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::DigitalSignature];
    params
}

fn sequence(content: Vec<u8>) -> Vec<u8> {
    let mut result = vec![0x30];
    if content.len() < 128 { result.push(u8::try_from(content.len()).unwrap()); }
    else { result.extend_from_slice(&[0x81, u8::try_from(content.len()).unwrap()]); }
    result.extend(content);
    result
}

async fn tls_fixture(issuer: &Issuer<'_, KeyPair>, aia: &str, records: Arc<Mutex<Vec<Vec<u8>>>>) -> (u16, tokio::task::JoinHandle<()>) {
    let key = KeyPair::generate().unwrap();
    let mut leaf = CertificateParams::new(vec!["localhost".into()]).unwrap();
    let mut access = vec![0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x02, 0x86, u8::try_from(aia.len()).unwrap()];
    access.extend_from_slice(aia.as_bytes());
    leaf.custom_extensions.push(CustomExtension::from_oid_content(&[1, 3, 6, 1, 5, 5, 7, 1, 1], sequence(sequence(access))));
    let leaf = leaf.signed_by(&key, issuer).unwrap();
    let config = tokio_rustls::rustls::ServerConfig::builder_with_provider(Arc::new(tokio_rustls::rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions().unwrap().with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(leaf.der().to_vec())], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()))).unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            let records = records.clone();
            tokio::spawn(async move {
                let Ok(mut socket) = acceptor.accept(socket).await else { return; };
                let mut request = Vec::new();
                loop {
                    let mut bytes = [0; 4096];
                    let Ok(size) = socket.read(&mut bytes).await else { return; };
                    if size == 0 { return; }
                    request.extend_from_slice(&bytes[..size]);
                    if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                        let length = headers.lines().find_map(|line| line.strip_prefix("content-length:").and_then(|value| value.trim().parse::<usize>().ok())).unwrap_or(0);
                        if request.len() >= end + 4 + length { break; }
                    }
                }
                let drop_response = request.starts_with(b"POST /drop ");
                records.lock().unwrap().push(request);
                if !drop_response {
                    let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nverified").await;
                }
            });
        }
    });
    (port, task)
}
