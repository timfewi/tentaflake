use super::*;
use std::sync::{Arc, Barrier, Mutex};

struct Secrets;
impl CredentialSource for Secrets {
    fn load(
        &self,
        reference: &str,
        _: Provider,
        tenant: &str,
        account: &str,
    ) -> Result<Credential, Error> {
        assert_eq!(
            (reference, tenant, account),
            ("runtime-key", "tenant", "account")
        );
        Credential::new("synthetic-secret-canary".into())
    }
}
#[derive(Clone, Default)]
struct AuditLog(Arc<Mutex<Vec<Audit>>>);
impl AuditSink for AuditLog {
    fn record(&self, event: Audit) {
        self.0.lock().unwrap().push(event);
    }
}
struct Fixture {
    status: u16,
    body: Vec<u8>,
    calls: AtomicUsize,
}
impl Transport for Fixture {
    fn get(&self, url: &Url, _: &Credential, _: usize, _: Duration) -> Result<Response, Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(url.host_str(), Some("www.googleapis.com"));
        Ok(Response {
            status: self.status,
            body: self.body.clone(),
        })
    }
}
fn grant() -> Grant {
    Grant {
        agent: "agent-a".into(),
        tenant: "tenant".into(),
        account: "account".into(),
        provider: Provider::Drive,
        resources: BTreeSet::from(["file".into()]),
        methods: BTreeSet::from([ReadMethod::DriveMetadata]),
        credential_reference: "runtime-key".into(),
        generation: 3,
        expires_unix: u64::MAX,
        max_body_bytes: 128,
        max_concurrency: 1,
        deadline: Duration::from_secs(5),
    }
}
fn request() -> ReadRequest {
    ReadRequest {
        tenant: "tenant".into(),
        account: "account".into(),
        resource: "file".into(),
        method: ReadMethod::DriveMetadata,
        generation: 3,
    }
}
fn gateway(status: u16, body: &[u8]) -> Gateway<Fixture, Secrets, AuditLog> {
    Gateway::new(
        grant(),
        Fixture {
            status,
            body: body.into(),
            calls: AtomicUsize::new(0),
        },
        Secrets,
        AuditLog::default(),
    )
    .unwrap()
}
#[test]
fn grant_checks_precede_transport() {
    let g = gateway(200, br#"{"id":"file"}"#);
    for field in 0..5 {
        let mut r = request();
        match field {
            0 => r.tenant = "other".into(),
            1 => r.account = "other".into(),
            2 => r.resource = "other".into(),
            3 => r.method = ReadMethod::DriveFile,
            _ => r.generation = 2,
        }
        assert!(matches!(g.read(&r), Err(Error::Denied)));
    }
    assert_eq!(g.transport.calls.load(Ordering::SeqCst), 0);
    assert!(g.read(&request()).is_ok());
    g.revoke();
    assert!(matches!(g.read(&request()), Err(Error::Revoked)));
    let mut p = grant();
    p.expires_unix = 0;
    let expired = Gateway::new(
        p,
        Fixture {
            status: 200,
            body: vec![],
            calls: AtomicUsize::new(0),
        },
        Secrets,
        AuditLog::default(),
    )
    .unwrap();
    assert!(matches!(expired.read(&request()), Err(Error::Expired)));
}
#[test]
fn injected_authority_and_writes_are_not_request_fields() {
    for field in [
        "credential",
        "scope",
        "endpoint",
        "policy",
        "url",
        "http_method",
        "replay",
        "page_token",
    ] {
        let mut value = serde_json::json!({"tenant":"tenant","account":"account","resource":"file","method":"drive_metadata","generation":3});
        value[field] = "attack".into();
        assert!(serde_json::from_value::<ReadRequest>(value).is_err());
    }
    assert!(serde_json::from_str::<ReadMethod>("\"gmail_send\"").is_err());
    assert!(serde_json::from_str::<ReadMethod>("\"hub_spot_update\"").is_err());
}
#[test]
fn routes_encode_arguments_as_single_segments() {
    let mut r = request();
    r.resource = "a/b?x=#%".into();
    assert_eq!(
        route(&r).unwrap().as_str(),
        "https://www.googleapis.com/drive/v3/files/a%2Fb%3Fx=%23%25?fields=id%2Cname%2CmimeType%2Csize%2CmodifiedTime"
    );
    r.method = ReadMethod::GmailMessage;
    r.account = "user@example.test".into();
    r.resource = "abc".into();
    assert_eq!(
        route(&r).unwrap().as_str(),
        "https://gmail.googleapis.com/gmail/v1/users/user@example.test/messages/abc?format=full"
    );
    r.method = ReadMethod::HubSpotContact;
    assert_eq!(
        route(&r).unwrap().as_str(),
        "https://api.hubapi.com/crm/v3/objects/contacts/abc?properties=email%2Cfirstname%2Clastname&archived=false"
    );
    r.method = ReadMethod::DriveFile;
    assert_eq!(route(&r).unwrap().query(), Some("alt=media"));
    r.resource = "..".into();
    assert!(route(&r).is_err());
}
#[test]
fn malformed_oversized_redirect_and_failure_never_retry() {
    for (status, body, expected) in [
        (200, b"not-json".to_vec(), Error::Malformed),
        (200, vec![b'x'; 129], Error::TooLarge),
        (302, vec![], Error::Redirect),
        (503, vec![], Error::ProviderFailure),
        (200, br#"{"id":"other"}"#.to_vec(), Error::Malformed),
    ] {
        let g = gateway(status, &body);
        assert!(matches!(g.read(&request()), Err(e) if e == expected));
        assert_eq!(g.transport.calls.load(Ordering::SeqCst), 1);
    }
}
#[test]
fn public_address_and_origin_fail_closed_without_network() {
    for address in [
        "127.0.0.1",
        "10.0.0.1",
        "169.254.169.254",
        "100.64.0.1",
        "192.168.0.1",
        "198.18.0.1",
        "::1",
        "::ffff:8.8.8.8",
        "fc00::1",
        "2001:db8::1",
        "2002:0808:0808::1",
    ] {
        assert!(!public_address(address.parse().unwrap()), "{address}");
        assert!(HttpsTransport::new(Provider::Drive, &[address.parse().unwrap()]).is_err());
    }
    assert!(public_address("8.8.8.8".parse().unwrap()));
    let t = HttpsTransport::new(Provider::Drive, &["8.8.8.8".parse().unwrap()]).unwrap();
    let credential = Credential::new("synthetic".into()).unwrap();
    for url in [
        "http://www.googleapis.com/",
        "https://localhost/",
        "https://www.googleapis.com:444/",
        "https://user@www.googleapis.com/",
    ] {
        assert!(matches!(
            t.get(
                &Url::parse(url).unwrap(),
                &credential,
                10,
                Duration::from_secs(1)
            ),
            Err(Error::Denied)
        ));
    }
}
#[test]
fn audit_contains_no_credentials_identifiers_or_content() {
    let g = gateway(200, br#"{"id":"file","name":"content-canary"}"#);
    assert!(g.read(&request()).is_ok());
    let audit = serde_json::to_string(&*g.audit.0.lock().unwrap()).unwrap();
    for forbidden in [
        "synthetic-secret-canary",
        "content-canary",
        "runtime-key",
        "tenant",
        "account",
        "googleapis",
    ] {
        assert!(!audit.contains(forbidden));
    }
}
struct Blocking {
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
}
impl Transport for Blocking {
    fn get(&self, _: &Url, _: &Credential, _: usize, _: Duration) -> Result<Response, Error> {
        self.entered.wait();
        self.release.wait();
        Ok(Response {
            status: 200,
            body: br#"{"id":"file"}"#.to_vec(),
        })
    }
}
#[test]
fn concurrency_and_inflight_revocation_are_enforced() {
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let g = Arc::new(
        Gateway::new(
            grant(),
            Blocking {
                entered: entered.clone(),
                release: release.clone(),
            },
            Secrets,
            AuditLog::default(),
        )
        .unwrap(),
    );
    let other = g.clone();
    let task = std::thread::spawn(move || other.read(&request()));
    entered.wait();
    assert!(matches!(g.read(&request()), Err(Error::Busy)));
    g.revoke();
    release.wait();
    assert!(matches!(task.join().unwrap(), Err(Error::Revoked)));
    assert_eq!(g.active.load(Ordering::SeqCst), 0);
}

struct AdapterFixture {
    expected: Url,
    body: Vec<u8>,
}
impl Transport for AdapterFixture {
    fn get(&self, url: &Url, _: &Credential, _: usize, _: Duration) -> Result<Response, Error> {
        assert_eq!(url, &self.expected);
        Ok(Response {
            status: 200,
            body: self.body.clone(),
        })
    }
}
#[test]
fn every_adapter_uses_authorized_route_and_bounded_response() {
    for method in [
        ReadMethod::DriveMetadata,
        ReadMethod::DriveFile,
        ReadMethod::GmailMessage,
        ReadMethod::HubSpotContact,
    ] {
        let mut p = grant();
        p.provider = method.provider();
        p.methods = BTreeSet::from([method]);
        let mut r = request();
        r.method = method;
        let body = if method == ReadMethod::DriveFile {
            vec![0, 255, 42]
        } else {
            br#"{"id":"file"}"#.to_vec()
        };
        let g = Gateway::new(
            p,
            AdapterFixture {
                expected: route(&r).unwrap(),
                body: body.clone(),
            },
            Secrets,
            AuditLog::default(),
        )
        .unwrap();
        match g.read(&r).unwrap() {
            Content::File(value) => assert_eq!(value, body),
            Content::Json(value) => assert_eq!(value["id"], "file"),
        }
        assert_eq!(g.audit.0.lock().unwrap().len(), 1);
    }
}

struct NoCredentialAccess;
impl CredentialSource for NoCredentialAccess {
    fn load(&self, _: &str, _: Provider, _: &str, _: &str) -> Result<Credential, Error> {
        panic!("unauthorized request reached credential channel");
    }
}
#[test]
fn wrong_tenant_and_account_never_load_credentials() {
    let g = Gateway::new(
        grant(),
        Fixture {
            status: 200,
            body: vec![],
            calls: AtomicUsize::new(0),
        },
        NoCredentialAccess,
        AuditLog::default(),
    )
    .unwrap();
    let mut r = request();
    r.tenant = "other".into();
    assert!(matches!(g.read(&r), Err(Error::Denied)));
    r = request();
    r.account = "other".into();
    assert!(matches!(g.read(&r), Err(Error::Denied)));
    assert_eq!(g.transport.calls.load(Ordering::SeqCst), 0);
}
