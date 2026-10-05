mod application;
mod config;
mod domain;
mod infrastructure;

use anyhow::Result;
use application::synchronize::{SyncSummary, SynchronizeWords};
use config::Config;
use infrastructure::cheatsheet_api::CheatsheetApi;
use infrastructure::keycloak::JwtKeycloakContextHolder;
use infrastructure::openrussian::OpenRussianClient;
use std::process::ExitCode;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    init_logging();
    match run() {
        Ok(summary) => {
            log_summary(&summary);
            if summary.fatal_error.is_some() || summary.failed > 0 {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "Intake failed");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<SyncSummary> {
    let config = Config::from_env()?;
    tracing::info!("Starting Russian Cheatsheet intake");
    let source = OpenRussianClient::from_share_url(&config.openrussian_url)?;
    let keycloak = Arc::new(JwtKeycloakContextHolder::new(
        &config.keycloak_token_url,
        config.keycloak_client_id,
        config.keycloak_client_secret,
    )?);
    keycloak.get_token()?;
    let api = CheatsheetApi::new(&config.api_base_url, keycloak)?;
    SynchronizeWords::new(source, api).run()
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339())
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}

fn log_summary(summary: &SyncSummary) {
    tracing::info!(
        scanned = summary.scanned,
        disabled_skipped = summary.disabled_skipped,
        created = summary.created,
        updated = summary.updated,
        unchanged = summary.unchanged,
        failed = summary.failed,
        "Intake summary"
    );
    if let Some(error) = &summary.fatal_error {
        tracing::error!(error = %error, "Intake stopped");
    }
}
