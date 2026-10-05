use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use reqwest::blocking::Client;
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, info};
use url::Url;

const EARLY_REFRESH_WINDOW: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

pub struct JwtKeycloakContextHolder {
    http: Client,
    token_url: Url,
    client_id: String,
    client_secret: String,
    cached_token: Mutex<Option<CachedToken>>,
}

struct CachedToken {
    access_token: String,
    expires_at: SystemTime,
}

impl JwtKeycloakContextHolder {
    pub fn new(token_url: &str, client_id: String, client_secret: String) -> Result<Self> {
        let token_url = Url::parse(token_url).context("invalid KEYCLOAK_TOKEN_URL")?;
        if !matches!(token_url.scheme(), "http" | "https") {
            bail!("KEYCLOAK_TOKEN_URL must use HTTP or HTTPS");
        }

        let http = Client::builder()
            .user_agent(concat!(
                "russian-cheatsheet-intake/",
                env!("CARGO_PKG_VERSION")
            ))
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("could not create Keycloak HTTP client")?;

        Ok(Self {
            http,
            token_url,
            client_id,
            client_secret,
            cached_token: Mutex::new(None),
        })
    }

    pub fn get_token(&self) -> Result<String> {
        let mut cached_token = self
            .cached_token
            .lock()
            .map_err(|_| anyhow::anyhow!("Keycloak token cache lock poisoned"))?;
        let refresh_at = SystemTime::now()
            .checked_add(EARLY_REFRESH_WINDOW)
            .context("system clock overflow while checking Keycloak token expiry")?;

        if let Some(token) = cached_token.as_ref() {
            if token.expires_at > refresh_at {
                let seconds_remaining = token
                    .expires_at
                    .duration_since(SystemTime::now())
                    .map_or(0, |remaining| remaining.as_secs());
                debug!(seconds_remaining, "Reusing cached Keycloak access token");
                return Ok(token.access_token.clone());
            }
        }

        info!("Requesting new Keycloak access token with client credentials");
        let response: TokenResponse = self
            .http
            .post(self.token_url.clone())
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
            ])
            .send()
            .context("failed to request Keycloak access token")?
            .error_for_status()
            .context("Keycloak token endpoint rejected client credentials")?
            .json()
            .context("invalid Keycloak token response")?;

        if response.access_token.is_empty() {
            bail!("Keycloak returned an empty access token");
        }

        let received_at = SystemTime::now();
        let expires_at =
            token_expiration(&response.access_token, response.expires_in, received_at)?;
        let refresh_at = received_at
            .checked_add(EARLY_REFRESH_WINDOW)
            .context("system clock overflow while checking Keycloak token expiry")?;
        if expires_at <= refresh_at {
            bail!("Keycloak returned an access token that expires within 30 seconds");
        }

        let expires_at_unix = expires_at
            .duration_since(UNIX_EPOCH)
            .context("Keycloak token expiry predates Unix epoch")?
            .as_secs();
        let access_token = response.access_token;
        *cached_token = Some(CachedToken {
            access_token: access_token.clone(),
            expires_at,
        });
        info!(expires_at_unix, "Keycloak access token cached");
        Ok(access_token)
    }
}

fn token_expiration(
    token: &str,
    expires_in: Option<u64>,
    received_at: SystemTime,
) -> Result<SystemTime> {
    let jwt_expiration = jwt_expiration(token);
    let response_expiration = expires_in
        .map(|seconds| {
            received_at
                .checked_add(Duration::from_secs(seconds))
                .context("Keycloak expires_in value exceeds system clock range")
        })
        .transpose()?;

    match (jwt_expiration, response_expiration) {
        (Some(jwt_expiration), Some(response_expiration)) => {
            Ok(jwt_expiration.min(response_expiration))
        }
        (Some(jwt_expiration), None) => Ok(jwt_expiration),
        (None, Some(response_expiration)) => Ok(response_expiration),
        (None, None) => bail!("Keycloak token response has no usable expiry"),
    }
}

fn jwt_expiration(token: &str) -> Option<SystemTime> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| URL_SAFE.decode(payload))
        .ok()?;
    let claims: JwtClaims = serde_json::from_slice(&decoded).ok()?;
    UNIX_EPOCH.checked_add(Duration::from_secs(claims.exp?))
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct JwtClaims {
    exp: Option<u64>,
}
