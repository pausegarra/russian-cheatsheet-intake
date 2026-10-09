mod application;
mod config;
mod domain;
mod infrastructure;

use anyhow::Result;
use application::ports::WordSource;
use application::synchronize::{SyncSummary, SynchronizeWords};
use application::synchronize_examples::{ExampleSyncSummary, SynchronizeExamples};
use application::synchronize_word_relations::{SynchronizeWordRelations, WordRelationSyncSummary};
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
                || summary.relations.fatal_error.is_some()
                || summary.examples.fatal_error.is_some()
                || summary.words.failed + summary.relations.failed + summary.examples.failed > 0
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
    relations: WordRelationSyncSummary,
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
    let relation_snapshot = source.load_word_relation_snapshot()?;
    let example_snapshot = source.load_example_snapshot()?;
    let batch_size = config.batch_size;
    let word_synchronizer = SynchronizeWords::new(source, api.clone(), batch_size);
    let mut word_result = word_synchronizer.run_with_words()?;
    let (relations, examples) = if word_result.summary.fatal_error.is_some() {
        (
            WordRelationSyncSummary::default(),
            ExampleSyncSummary::default(),
        )
    } else {
        let relations = SynchronizeWordRelations::new(api.clone(), batch_size).run(
            relation_snapshot,
            &word_result.words_by_external_id,
            &word_result.active_source_word_ids,
        )?;
        let examples = SynchronizeExamples::new(api.clone(), batch_size)
            .run(example_snapshot, &word_result.words_by_external_id)?;
        if relations.fatal_error.is_none() && examples.fatal_error.is_none() {
            word_synchronizer
                .delete_stale_words(&word_result.stale_words, &mut word_result.summary);
        } else if !word_result.stale_words.is_empty() {
            tracing::warn!(
                stale_words = word_result.stale_words.len(),
                "Stale word deletion skipped because relation or example cleanup failed"
            );
        }
        (relations, examples)
    };
    Ok(IntakeSummary {
        words: word_result.summary,
        relations,
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
        words_deleted = summary.words.deleted,
        words_failed = summary.words.failed,
        word_relations_scanned = summary.relations.scanned,
        word_relations_candidates = summary.relations.candidates,
        word_relations_self_skipped = summary.relations.self_relations_skipped,
        word_relations_unmapped_skipped = summary.relations.unmapped_skipped,
        word_relations_created = summary.relations.created,
        word_relations_unchanged = summary.relations.unchanged,
        word_relations_deleted = summary.relations.deleted,
        word_relations_failed = summary.relations.failed,
        examples_scanned = summary.examples.scanned,
        examples_disabled_skipped = summary.examples.disabled_skipped,
        examples_unlinked_skipped = summary.examples.unlinked_skipped,
        examples_created = summary.examples.created,
        examples_updated = summary.examples.updated,
        examples_unchanged = summary.examples.unchanged,
        examples_deleted = summary.examples.deleted,
        examples_failed = summary.examples.failed,
        "Intake summary"
    );
    if let Some(error) = &summary.words.fatal_error {
        tracing::error!(error = %error, "Word intake stopped");
    }
    if let Some(error) = &summary.relations.fatal_error {
        tracing::error!(error = %error, "Word relation intake stopped");
    }
    if let Some(error) = &summary.examples.fatal_error {
        tracing::error!(error = %error, "Intake stopped");
    }
}
