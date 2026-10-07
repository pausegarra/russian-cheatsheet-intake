mod application;
mod config;
mod domain;
mod infrastructure;

use anyhow::Result;
use application::ports::WordSource;
use application::synchronize::{SyncSummary, SynchronizeWords};
use application::synchronize_examples::{ExampleSyncSummary, SynchronizeExamples};
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
            if summary.words.fatal_error.is_some()
                || summary.examples.fatal_error.is_some()
                || summary.words.failed + summary.examples.failed > 0
            {
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

struct IntakeSummary {
    words: SyncSummary,
    examples: ExampleSyncSummary,
}

fn run() -> Result<IntakeSummary> {
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
    let example_snapshot = source.load_example_snapshot()?;
    let word_result = SynchronizeWords::new(source, api.clone()).run_with_words()?;
    let examples = if word_result.summary.fatal_error.is_some() {
        ExampleSyncSummary::default()
    } else {
        SynchronizeExamples::new(api).run(example_snapshot, &word_result.words_by_external_id)?
    };
    Ok(IntakeSummary {
        words: word_result.summary,
        examples,
    })
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

fn log_summary(summary: &IntakeSummary) {
    tracing::info!(
        words_scanned = summary.words.scanned,
        words_disabled_skipped = summary.words.disabled_skipped,
        words_created = summary.words.created,
        words_updated = summary.words.updated,
        words_unchanged = summary.words.unchanged,
        words_failed = summary.words.failed,
        examples_scanned = summary.examples.scanned,
        examples_disabled_skipped = summary.examples.disabled_skipped,
        examples_unlinked_skipped = summary.examples.unlinked_skipped,
        examples_created = summary.examples.created,
        examples_updated = summary.examples.updated,
        examples_unchanged = summary.examples.unchanged,
        examples_failed = summary.examples.failed,
        "Intake summary"
    );
    if let Some(error) = &summary.words.fatal_error {
        tracing::error!(error = %error, "Word intake stopped");
    }
    if let Some(error) = &summary.examples.fatal_error {
        tracing::error!(error = %error, "Intake stopped");
    }
}
