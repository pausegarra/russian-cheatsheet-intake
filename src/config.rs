use anyhow::{Context, Result};
use std::env;
use std::path::Path;

mod embedded_env {
    include!(concat!(env!("OUT_DIR"), "/embedded_env.rs"));
}

const DEFAULT_OPENRUSSIAN_URL: &str =
    "https://app.togetherdb.com/db/fwoedz5fvtwvq03v/openrussian_public/words";
const DEFAULT_BATCH_SIZE: usize = 1000;

pub struct ApiConfig {
    pub api_base_url: String,
    pub keycloak_token_url: String,
    pub keycloak_client_id: String,
    pub keycloak_client_secret: String,
}

pub struct Config {
    pub api: Option<ApiConfig>,
    pub openrussian_url: String,
    pub batch_size: usize,
}

impl Config {
    pub fn from_env(dry_run: bool) -> Result<Self> {
        if Path::new(".env").is_file() {
            dotenvy::dotenv().context("could not load .env")?;
        }
        if !embedded_env::EMBEDDED_ENV.is_empty() {
            dotenvy::from_read(embedded_env::EMBEDDED_ENV.as_bytes())
                .context("could not load embedded .env")?;
        }

        let api = if dry_run {
            None
        } else {
            Some(ApiConfig {
                api_base_url: required_env(
                    "RUSSIAN_CHEATSHEET_API_URL",
                    "for example: http://localhost:8080/api",
                )?,
                keycloak_token_url: required_env(
                    "KEYCLOAK_TOKEN_URL",
                    "full OpenID Connect token endpoint URL",
                )?,
                keycloak_client_id: required_env("KEYCLOAK_CLIENT_ID", "")?,
                keycloak_client_secret: required_env("KEYCLOAK_CLIENT_SECRET", "")?,
            })
        };
        if let Some(api) = &api {
            for (name, value) in [
                ("RUSSIAN_CHEATSHEET_API_URL", &api.api_base_url),
                ("KEYCLOAK_TOKEN_URL", &api.keycloak_token_url),
                ("KEYCLOAK_CLIENT_ID", &api.keycloak_client_id),
                ("KEYCLOAK_CLIENT_SECRET", &api.keycloak_client_secret),
            ] {
                if value.trim().is_empty() {
                    anyhow::bail!("{name} must not be empty");
                }
            }
        }

        let openrussian_url = env::var("OPENRUSSIAN_TOGETHERDB_URL")
            .unwrap_or_else(|_| DEFAULT_OPENRUSSIAN_URL.to_owned());
        let batch_size = if dry_run {
            DEFAULT_BATCH_SIZE
        } else {
            match env::var("RUSSIAN_CHEATSHEET_BATCH_SIZE") {
                Ok(value) => value
                    .parse::<usize>()
                    .context("RUSSIAN_CHEATSHEET_BATCH_SIZE must be an unsigned integer")?,
                Err(env::VarError::NotPresent) => DEFAULT_BATCH_SIZE,
                Err(error) => {
                    return Err(error).context("could not read RUSSIAN_CHEATSHEET_BATCH_SIZE");
                }
            }
        };

        Ok(Self {
            api,
            openrussian_url,
            batch_size,
        })
    }
}

fn required_env(name: &str, hint: &str) -> Result<String> {
    let message = if hint.is_empty() {
        format!("{name} is required")
    } else {
        format!("{name} is required ({hint})")
    };
    env::var(name).with_context(|| message)
}
