//! Tenant-specific Entra delegated access tokens. Keys never come from JWT URLs.
use crate::security::{Identity, Role};
use axum::{Json, extract::State, http::StatusCode};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntraConfig {
    pub tenant_id: String,
    pub client_id: String,
    pub required_scope: String,
    pub principals: HashMap<String, Role>,
    #[serde(skip)]
    cache: Arc<Mutex<KeyCache>>,
}
#[derive(Debug, Default)]
struct KeyCache {
    keys: Option<JwkSet>,
    loaded: Option<Instant>,
    attempted: Option<Instant>,
}
#[derive(Clone, Deserialize)]
struct Claims {
    tid: String,
    oid: String,
    scp: String,
    // Typed required claims also prevent accepting malformed NumericDate values.
    exp: u64,
    nbf: u64,
}
impl EntraConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        for id in [&self.tenant_id, &self.client_id] {
            anyhow::ensure!(
                uuid::Uuid::parse_str(id)?.to_string() == *id,
                "Entra IDs must be canonical UUIDs"
            );
        }
        anyhow::ensure!(
            self.required_scope == "access_as_user",
            "Entra requires access_as_user delegated scope"
        );
        anyhow::ensure!(
            !self.principals.is_empty(),
            "Entra requires an explicit principal allowlist"
        );
        for oid in self.principals.keys() {
            anyhow::ensure!(
                uuid::Uuid::parse_str(oid)?.to_string() == *oid,
                "Entra principal IDs must be canonical UUIDs"
            );
        }
        Ok(())
    }
    fn verify(&self, token: &str, keys: &JwkSet) -> Result<Identity, StatusCode> {
        let header = decode_header(token).map_err(|_| StatusCode::UNAUTHORIZED)?;
        if header.alg != Algorithm::RS256 {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let kid = header.kid.ok_or(StatusCode::UNAUTHORIZED)?;
        let jwk = keys.find(&kid).ok_or(StatusCode::UNAUTHORIZED)?;
        let key = DecodingKey::from_jwk(jwk).map_err(|_| StatusCode::UNAUTHORIZED)?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[&self.client_id]);
        validation.set_issuer(&[format!(
            "https://login.microsoftonline.com/{}/v2.0",
            self.tenant_id
        )]);
        validation.set_required_spec_claims(&["exp", "nbf", "aud", "iss"]);
        validation.validate_nbf = true;
        validation.leeway = 30;
        let claims = decode::<Claims>(token, &key, &validation)
            .map_err(|_| StatusCode::UNAUTHORIZED)?
            .claims;
        if claims.tid != self.tenant_id
            || claims.exp <= claims.nbf
            || !claims
                .scp
                .split_whitespace()
                .any(|scope| scope == self.required_scope)
        {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let role = self
            .principals
            .get(&claims.oid)
            .copied()
            .ok_or(StatusCode::FORBIDDEN)?;
        Ok(Identity {
            principal: format!("entra:{}:{}", claims.tid, claims.oid),
            role,
        })
    }
    pub async fn authenticate(&self, token: &str) -> Result<Identity, StatusCode> {
        if token.len() > 16384 {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let header = decode_header(token).map_err(|_| StatusCode::UNAUTHORIZED)?;
        if header.alg != Algorithm::RS256
            || header
                .kid
                .as_ref()
                .is_none_or(|kid| kid.is_empty() || kid.len() > 256)
        {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let mut cache = self.cache.lock().await;
        let fresh = cache
            .loaded
            .is_some_and(|time| time.elapsed() < Duration::from_secs(3600));
        let known = cache
            .keys
            .as_ref()
            .is_some_and(|keys| keys.find(header.kid.as_deref().unwrap()).is_some());
        if !fresh || !known {
            // Single-flight refresh, globally rate limited per configuration even for unknown kids.
            if cache
                .attempted
                .is_none_or(|time| time.elapsed() >= Duration::from_secs(60))
            {
                cache.attempted = Some(Instant::now());
                if let Ok(keys) = self.fetch_keys().await {
                    cache.keys = Some(keys);
                    cache.loaded = Some(Instant::now());
                }
            }
        }
        if !cache
            .loaded
            .is_some_and(|time| time.elapsed() < Duration::from_secs(3600))
        {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        self.verify(
            token,
            cache.keys.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?,
        )
    }
    async fn fetch_keys(&self) -> anyhow::Result<JwkSet> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let mut response = client
            .get(format!(
                "https://login.microsoftonline.com/{}/discovery/v2.0/keys",
                self.tenant_id
            ))
            .send()
            .await?
            .error_for_status()?;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            anyhow::ensure!(
                body.len() + chunk.len() <= 262144,
                "Entra JWKS exceeds size limit"
            );
            body.extend_from_slice(&chunk);
        }
        let keys: JwkSet = serde_json::from_slice(&body)?;
        anyhow::ensure!(
            !keys.keys.is_empty() && keys.keys.len() <= 64,
            "Invalid Entra JWKS key count"
        );
        Ok(keys)
    }
}
pub async fn public_config(State(state): State<Arc<crate::AppState>>) -> Json<serde_json::Value> {
    Json(
        serde_json::json!({"entra": state.config.security.entra.as_ref().map(|config| serde_json::json!({
            "tenant_id": config.tenant_id, "client_id": config.client_id,
            "scope": format!("api://{}/{}", config.client_id, config.required_scope)
        }))}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use jsonwebtoken::{EncodingKey, Header, encode};
    use rsa::{
        pkcs8::{EncodePrivateKey, LineEnding},
        traits::PublicKeyParts,
    };
    use std::sync::LazyLock;
    const TENANT: &str = "11111111-1111-1111-1111-111111111111";
    const CLIENT: &str = "22222222-2222-2222-2222-222222222222";
    const OID: &str = "33333333-3333-3333-3333-333333333333";
    static RSA: LazyLock<rsa::RsaPrivateKey> =
        LazyLock::new(|| rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap());
    fn setup() -> (EntraConfig, JwkSet, serde_json::Value) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let config = EntraConfig {
            tenant_id: TENANT.into(),
            client_id: CLIENT.into(),
            required_scope: "access_as_user".into(),
            principals: HashMap::from([(OID.into(), Role::Analyst)]),
            cache: Default::default(),
        };
        let keys = serde_json::from_value(serde_json::json!({"keys":[{"kty":"RSA","kid":"test","alg":"RS256","use":"sig","n":URL_SAFE_NO_PAD.encode(RSA.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(RSA.e().to_bytes_be())}]})).unwrap();
        (
            config,
            keys,
            serde_json::json!({"iss":format!("https://login.microsoftonline.com/{TENANT}/v2.0"),"aud":CLIENT,"tid":TENANT,"oid":OID,"scp":"access_as_user","exp":now+3600,"nbf":now-60}),
        )
    }
    fn sign(claims: &serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test".into());
        encode(
            &header,
            claims,
            &EncodingKey::from_rsa_pem(RSA.to_pkcs8_pem(LineEnding::LF).unwrap().as_bytes())
                .unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn signed_delegated_tokens_require_all_security_claims() {
        let (config, keys, claims) = setup();
        config.validate().unwrap();
        let identity = config.verify(&sign(&claims), &keys).unwrap();
        assert_eq!(identity.role, Role::Analyst);
        assert_eq!(identity.principal, format!("entra:{TENANT}:{OID}"));
        for (field, value) in [
            ("aud", serde_json::json!("https://management.azure.com/")),
            ("iss", serde_json::json!("https://attacker.invalid/v2.0")),
            ("tid", serde_json::json!(CLIENT)),
            ("exp", serde_json::json!(1)),
            ("nbf", serde_json::json!(4102444800_u64)),
            ("scp", serde_json::json!("User.Read")),
            ("oid", serde_json::json!(CLIENT)),
            ("exp", serde_json::json!("bad")),
        ] {
            let mut invalid = claims.clone();
            invalid[field] = value;
            assert!(
                config.verify(&sign(&invalid), &keys).is_err(),
                "accepted invalid {field}"
            );
        }
        for field in ["aud", "iss", "tid", "oid", "scp", "exp", "nbf"] {
            let mut invalid = claims.clone();
            invalid.as_object_mut().unwrap().remove(field);
            assert!(
                config.verify(&sign(&invalid), &keys).is_err(),
                "accepted absent {field}"
            );
        }
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("test".into());
        let confused = encode(&header, &claims, &EncodingKey::from_secret(b"test-secret")).unwrap();
        assert!(config.verify(&confused, &keys).is_err());
        let mut token = sign(&claims).into_bytes();
        let last = token.len() - 10;
        token[last] = if token[last] == b'A' { b'B' } else { b'A' };
        assert!(
            config
                .verify(std::str::from_utf8(&token).unwrap(), &keys)
                .is_err()
        );
    }
    #[tokio::test]
    async fn cached_keys_validate_without_network_and_unknown_kids_are_throttled() {
        let (config, keys, claims) = setup();
        *config.cache.lock().await = KeyCache {
            keys: Some(keys),
            loaded: Some(Instant::now()),
            attempted: Some(Instant::now()),
        };
        assert!(config.authenticate(&sign(&claims)).await.is_ok());
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("unknown".into());
        let token = encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(RSA.to_pkcs8_pem(LineEnding::LF).unwrap().as_bytes())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            config.authenticate(&token).await.unwrap_err(),
            StatusCode::UNAUTHORIZED
        );
        config.cache.lock().await.loaded = Some(Instant::now() - Duration::from_secs(3601));
        assert_eq!(
            config.authenticate(&sign(&claims)).await.unwrap_err(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
