use anyhow::{Context, Result};
use std::env;
use std::path::Path;

const DEFAULT_OPENRUSSIAN_URL: &str =
    "https://app.togetherdb.com/db/fwoedz5fvtwvq03v/openrussian_public/words";

pub struct Config {
    pub api_base_url: String,
    pub keycloak_token_url: String,
    pub keycloak_client_id: String,
    pub keycloak_client_secret: String,
    pub openrussian_url: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        if Path::new(".env").is_file() {
            dotenvy::dotenv().context("could not load .env")?;
        }

        let api_base_url = env::var("RUSSIAN_CHEATSHEET_API_URL").context(
            "RUSSIAN_CHEATSHEET_API_URL is required (for example: http://localhost:8080/api)",
        )?;
        let keycloak_token_url = env::var("KEYCLOAK_TOKEN_URL")
            .context("KEYCLOAK_TOKEN_URL is required (full OpenID Connect token endpoint URL)")?;
        let keycloak_client_id =
            env::var("KEYCLOAK_CLIENT_ID").context("KEYCLOAK_CLIENT_ID is required")?;
        let keycloak_client_secret =
            env::var("KEYCLOAK_CLIENT_SECRET").context("KEYCLOAK_CLIENT_SECRET is required")?;
        let openrussian_url = env::var("OPENRUSSIAN_TOGETHERDB_URL")
            .unwrap_or_else(|_| DEFAULT_OPENRUSSIAN_URL.to_owned());

        for (name, value) in [
            ("RUSSIAN_CHEATSHEET_API_URL", &api_base_url),
            ("KEYCLOAK_TOKEN_URL", &keycloak_token_url),
            ("KEYCLOAK_CLIENT_ID", &keycloak_client_id),
            ("KEYCLOAK_CLIENT_SECRET", &keycloak_client_secret),
        ] {
            if value.trim().is_empty() {
                anyhow::bail!("{name} must not be empty");
            }
        }

        Ok(Self {
            api_base_url,
            keycloak_token_url,
            keycloak_client_id,
            keycloak_client_secret,
            openrussian_url,
        })
    }
}
