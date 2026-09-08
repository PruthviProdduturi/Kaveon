use crate::args::Options;
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::{Method, Url};
use serde::Deserialize;
use std::cell::RefCell;
use std::time::{Duration, Instant};

#[derive(Clone, Deserialize)]
struct Entra {
    tenant_id: String,
    client_id: String,
    scope: String,
}
#[derive(Deserialize)]
struct Config {
    entra: Option<Entra>,
}
#[derive(Deserialize)]
struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    #[serde(default = "default_interval")]
    interval: u64,
}
fn default_interval() -> u64 {
    5
}
#[derive(Deserialize)]
struct Token {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
}
struct Credentials {
    access_token: String,
    expires: Option<Instant>,
    refresh_token: Option<String>,
}
pub struct Session {
    client: Client,
    identity_client: Client,
    entra: Option<Entra>,
    credentials: RefCell<Option<Credentials>>,
}
impl Session {
    pub fn connect(options: &Options) -> Result<Self, String> {
        validate_server(&options.server)?;
        let mut builder = Client::builder()
            .timeout(options.timeout)
            .redirect(reqwest::redirect::Policy::none());
        if let Some(path) = &options.ca_cert {
            let pem = std::fs::read(path)
                .map_err(|e| format!("cannot read CA certificate {}: {e}", path.display()))?;
            let certificates = reqwest::Certificate::from_pem_bundle(&pem)
                .map_err(|_| "invalid PEM CA certificate".to_owned())?;
            if certificates.is_empty() {
                return Err("CA certificate file contains no certificates".into());
            }
            for certificate in certificates {
                builder = builder.add_root_certificate(certificate);
            }
        }
        let client = builder
            .build()
            .map_err(|e| format!("cannot initialize HTTP client: {e}"))?;
        // The Engine's custom CA must never extend trust for Microsoft sign-in.
        let identity_client = Client::builder()
            .timeout(options.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        let mut session = Self {
            client,
            identity_client,
            entra: None,
            credentials: RefCell::new(None),
        };
        if options.auth == "none" {
            return Ok(session);
        }
        if let Ok(token) = std::env::var("KAVEON_ACCESS_TOKEN") {
            if token.trim().is_empty() {
                return Err("KAVEON_ACCESS_TOKEN is empty".into());
            }
            *session.credentials.borrow_mut() = Some(Credentials {
                access_token: token,
                expires: None,
                refresh_token: None,
            });
            return Ok(session);
        }
        let response = session
            .client
            .get(format!(
                "{}/v1/auth/config",
                options.server.trim_end_matches('/')
            ))
            .send()
            .map_err(|e| format!("cannot discover Engine authentication: {e}"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND && options.auth == "auto" {
            return Ok(session);
        }
        if !response.status().is_success() {
            return Err(format!(
                "authentication discovery returned HTTP {}",
                response.status()
            ));
        }
        let config: Config = response
            .json()
            .map_err(|_| "invalid Engine authentication configuration")?;
        if let Some(entra) = config.entra {
            entra.validate()?;
            session.entra = Some(entra);
            session.ensure_token()?;
        } else if options.auth == "microsoft" {
            return Err("this Engine has no Microsoft sign-in configured".into());
        }
        Ok(session)
    }
    pub fn request(&self, method: Method, url: &str) -> Result<RequestBuilder, String> {
        self.ensure_token()?;
        let request = self.client.request(method, url);
        Ok(match self.credentials.borrow().as_ref() {
            Some(token) => request.bearer_auth(&token.access_token),
            None => request,
        })
    }
    fn ensure_token(&self) -> Result<(), String> {
        let Some(entra) = &self.entra else {
            return Ok(());
        };
        let refresh = {
            let credentials = self.credentials.borrow();
            if let Some(token) = credentials.as_ref() {
                if token.expires.is_none_or(|expires| Instant::now() < expires) {
                    return Ok(());
                }
                token.refresh_token.clone()
            } else {
                None
            }
        };
        let token = if let Some(refresh) = refresh {
            let response = self
                .identity_client
                .post(format!("{}/token", entra.authority()))
                .form(&[
                    ("grant_type", "refresh_token"),
                    ("client_id", &entra.client_id),
                    ("refresh_token", &refresh),
                    ("scope", &entra.scopes()),
                ])
                .send()
                .map_err(|_| "Microsoft token refresh connection failed")?;
            if response.status().is_success() {
                let mut token: Token = response
                    .json()
                    .map_err(|_| "invalid Microsoft token response")?;
                if token.refresh_token.is_none() {
                    token.refresh_token = Some(refresh);
                }
                token
            } else {
                let body: serde_json::Value = response.json().unwrap_or_default();
                if body["error"] != "invalid_grant" {
                    return Err("Microsoft token refresh failed; restart CLI to sign in".into());
                }
                device_login(
                    &self.identity_client,
                    entra,
                    &entra.authority(),
                    std::thread::sleep,
                )?
            }
        } else {
            device_login(
                &self.identity_client,
                entra,
                &entra.authority(),
                std::thread::sleep,
            )?
        };
        if token.access_token.is_empty() || token.expires_in == 0 {
            return Err("Microsoft returned an empty or expired access token".into());
        }
        let lifetime = token.expires_in.saturating_sub(60).clamp(1, 86_400);
        *self.credentials.borrow_mut() = Some(Credentials {
            access_token: token.access_token,
            expires: Some(Instant::now() + Duration::from_secs(lifetime)),
            refresh_token: token.refresh_token,
        });
        Ok(())
    }
}
fn validate_server(server: &str) -> Result<(), String> {
    let url = Url::parse(server).map_err(|_| "invalid Engine URL")?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(
            "Engine URL requires HTTPS (HTTP is allowed only for local development)".into(),
        );
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Engine URL must not contain credentials, query parameters, or fragments".into(),
        );
    }
    Ok(())
}
impl Entra {
    fn validate(&self) -> Result<(), String> {
        fn uuid(value: &str) -> bool {
            value.len() == 36
                && value.bytes().enumerate().all(|(i, b)| {
                    if [8, 13, 18, 23].contains(&i) {
                        b == b'-'
                    } else {
                        b.is_ascii_hexdigit()
                    }
                })
        }
        if !uuid(&self.tenant_id)
            || !uuid(&self.client_id)
            || self.scope != format!("api://{}/access_as_user", self.client_id)
        {
            return Err("invalid Engine Microsoft tenant, client ID, or delegated scope".into());
        }
        Ok(())
    }
    fn authority(&self) -> String {
        format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0",
            self.tenant_id
        )
    }
    fn scopes(&self) -> String {
        format!("{} offline_access", self.scope)
    }
}
fn device_login(
    client: &Client,
    entra: &Entra,
    authority: &str,
    mut sleep: impl FnMut(Duration),
) -> Result<Token, String> {
    let response = client
        .post(format!("{authority}/devicecode"))
        .form(&[("client_id", &entra.client_id), ("scope", &entra.scopes())])
        .send()
        .map_err(|_| "cannot connect to Microsoft sign-in")?;
    if !response.status().is_success() {
        return Err(format!(
            "Microsoft device sign-in returned HTTP {}; verify public client flows and tenant access",
            response.status()
        ));
    }
    let device: DeviceCode = response
        .json()
        .map_err(|_| "invalid Microsoft device-code response")?;
    // Display only Microsoft's known verification site, never an arbitrary URL from the Engine.
    if device.verification_uri != "https://microsoft.com/devicelogin"
        && device.verification_uri != "https://www.microsoft.com/devicelogin"
    {
        return Err("unexpected Microsoft device verification URL".into());
    }
    eprintln!(
        "Sign in at {} and enter code {}",
        device.verification_uri, device.user_code
    );
    let deadline = Instant::now() + Duration::from_secs(device.expires_in.min(1800));
    let mut interval = device.interval.clamp(1, 60);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining <= Duration::from_secs(interval) {
            return Err("Microsoft sign-in expired; run the CLI again".into());
        }
        sleep(Duration::from_secs(interval));
        let response = client
            .post(format!("{authority}/token"))
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", &entra.client_id),
                ("device_code", &device.device_code),
            ])
            .send()
            .map_err(|_| "Microsoft sign-in polling failed")?;
        if response.status().is_success() {
            return response
                .json()
                .map_err(|_| "invalid Microsoft token response".into());
        }
        let body: serde_json::Value = response
            .json()
            .map_err(|_| "invalid Microsoft sign-in response")?;
        match body["error"].as_str() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval = (interval + 5).min(120),
            Some("authorization_declined" | "access_denied") => {
                return Err("Microsoft sign-in was declined".into());
            }
            Some("expired_token") => {
                return Err("Microsoft sign-in expired; run the CLI again".into());
            }
            _ => {
                return Err(
                    "Microsoft sign-in failed; check tenant access and public client configuration"
                        .into(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    fn entra() -> Entra {
        Entra {
            tenant_id: "11111111-1111-1111-1111-111111111111".into(),
            client_id: "22222222-2222-2222-2222-222222222222".into(),
            scope: "api://22222222-2222-2222-2222-222222222222/access_as_user".into(),
        }
    }
    fn mock(responses: Vec<(u16, String)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut chunk = [0; 4096];
                    let n = stream.read(&mut chunk).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .map_or(0, |v| v.parse::<usize>().unwrap());
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                requests.push(String::from_utf8(bytes).unwrap());
                write!(stream, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            requests
        });
        (url, thread)
    }
    #[test]
    fn device_flow_polls_pending_and_slow_down_then_obtains_token() {
        let (url, thread) = mock(vec![
            (200, r#"{"device_code":"device-secret","user_code":"TEST","verification_uri":"https://microsoft.com/devicelogin","expires_in":900,"interval":2}"#.into()),
            (400, r#"{"error":"authorization_pending"}"#.into()),
            (400, r#"{"error":"slow_down"}"#.into()),
            (200, r#"{"access_token":"access-secret","expires_in":3600,"refresh_token":"refresh-secret"}"#.into()),
        ]);
        let mut waits = Vec::new();
        let token = device_login(&Client::new(), &entra(), &url, |delay| {
            waits.push(delay.as_secs())
        })
        .unwrap();
        assert_eq!(token.access_token, "access-secret");
        assert_eq!(waits, [2, 2, 7]);
        let requests = thread.join().unwrap();
        assert!(requests[0].contains("offline_access"));
        assert!(requests[1].contains("device_code=device-secret"));
        assert!(!requests[0].contains("access-secret"));
    }
    #[test]
    fn device_flow_denial_and_expiry_are_terminal() {
        for (expires, error, expected) in [(900, "access_denied", "declined"), (0, "", "expired")] {
            let mut responses = vec![(
                200,
                format!(
                    r#"{{"device_code":"x","user_code":"TEST","verification_uri":"https://microsoft.com/devicelogin","expires_in":{expires}}}"#
                ),
            )];
            if expires > 0 {
                responses.push((400, format!(r#"{{"error":"{error}"}}"#)));
            }
            let (url, thread) = mock(responses);
            let result = device_login(&Client::new(), &entra(), &url, |_| {});
            assert!(result.err().unwrap().contains(expected));
            thread.join().unwrap();
        }
    }
    #[test]
    fn bearer_applies_to_statement_and_catalog_requests() {
        let client = Client::new();
        let session = Session {
            client: client.clone(),
            identity_client: client,
            entra: None,
            credentials: RefCell::new(Some(Credentials {
                access_token: "test-token".into(),
                expires: None,
                refresh_token: None,
            })),
        };
        for path in ["/v1/statement", "/v1/catalog"] {
            let request = session
                .request(Method::GET, &format!("https://localhost:8080{path}"))
                .unwrap()
                .build()
                .unwrap();
            assert_eq!(request.headers()["authorization"], "Bearer test-token");
        }
    }
    #[test]
    fn discovery_cannot_redirect_identity_or_request_other_scopes() {
        let mut config = entra();
        config.validate().unwrap();
        config.tenant_id = "../common?url=evil".into();
        assert!(config.validate().is_err());
        let mut config = entra();
        config.scope = "https://graph.microsoft.com/.default".into();
        assert!(config.validate().is_err());
        assert!(validate_server("http://remote.example:8080").is_err());
        assert!(validate_server("https://user:password@example.com").is_err());
        assert!(validate_server("http://127.0.0.1:8080").is_ok());
    }
    #[test]
    fn remote_execution_uses_server_inline_protocol() {
        let (url, thread) = mock(vec![(200, r#"{"id":"query-1","state":"FINISHED","columns":[{"name":"answer","type":"Int64"}],"data":[[42]],"error":null,"elapsed_ms":1}"#.into())]);
        let crate::args::Command::Run(mut options) = crate::args::parse(&[
            "kaveon".into(),
            "--server".into(),
            url,
            "--auth".into(),
            "none".into(),
            "--execute".into(),
            "SELECT 42".into(),
            "--output-format".into(),
            "json".into(),
        ])
        .unwrap() else {
            panic!()
        };
        crate::remote::run(&mut options).unwrap();
        let requests = thread.join().unwrap();
        assert!(requests[0].starts_with("POST /v1/statement "));
        let body: serde_json::Value =
            serde_json::from_str(requests[0].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["result_delivery"], "inline");
        assert_eq!(body["query"], "SELECT 42");
    }
    #[test]
    fn rejects_invalid_ca_before_network() {
        let path =
            std::env::temp_dir().join(format!("kaveon-cli-bad-ca-{}.pem", std::process::id()));
        std::fs::write(&path, b"not a certificate").unwrap();
        let crate::args::Command::Run(mut options) =
            crate::args::parse(&["kaveon".into()]).unwrap()
        else {
            panic!()
        };
        options.ca_cert = Some(path.clone());
        let error = Session::connect(&options).err().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(error.contains("certificate"));
    }
}
