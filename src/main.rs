mod application;
mod config;
mod domain;
mod infrastructure;

use anyhow::{Context, Result};
use application::desired_state::{self, DesiredStateSummary};
use application::ports::{SpanishTranslationSource, WordSource};
use application::synchronize::{SyncSummary, SynchronizeWords};
use application::synchronize_examples::{ExampleSyncSummary, SynchronizeExamples};
use application::synchronize_word_relations::{SynchronizeWordRelations, WordRelationSyncSummary};
use config::{ApiConfig, Config};
use infrastructure::cheatsheet_api::CheatsheetApi;
use infrastructure::desired_state_file;
use infrastructure::keycloak::JwtKeycloakContextHolder;
use infrastructure::openrussian::OpenRussianClient;
use infrastructure::rest_clients::spanish_translations::SpanishTranslationProvider;
use std::process::ExitCode;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    init_logging();
    match parse_command() {
        Ok(Command::Help) => {
            println!("Usage: russian-cheatsheet-intake [--dry-run]");
            ExitCode::SUCCESS
        }
        Ok(command) => match run(matches!(command, Command::DryRun)) {
            Ok(RunSummary::Sync(summary)) => {
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
            Ok(RunSummary::DryRun(summary)) => {
                log_desired_state_summary(&summary);
                if summary.errors > 0 {
                    ExitCode::FAILURE
                } else {
                    ExitCode::SUCCESS
                }
            }
            Err(error) => {
                tracing::error!(error = %format!("{error:#}"), "Intake failed");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "Invalid command");
            ExitCode::FAILURE
        }
    }
}

enum Command {
    Run,
    DryRun,
    Help,
}

fn parse_command() -> Result<Command> {
    let mut dry_run = false;
    let mut help = false;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--dry-run" if !dry_run => dry_run = true,
            "--dry-run" => anyhow::bail!("--dry-run may only be specified once"),
            "--help" | "-h" => help = true,
            unknown => anyhow::bail!("unknown argument '{unknown}'; use --help for usage"),
        }
    }
    if help {
        Ok(Command::Help)
    } else if dry_run {
        Ok(Command::DryRun)
    } else {
        Ok(Command::Run)
    }
}

struct IntakeSummary {
    words: SyncSummary,
    relations: WordRelationSyncSummary,
    examples: ExampleSyncSummary,
}

enum RunSummary {
    Sync(IntakeSummary),
    DryRun(DesiredStateSummary),
}

fn run(dry_run: bool) -> Result<RunSummary> {
    let config = Config::from_env(dry_run)?;
    tracing::info!(dry_run, "Starting Russian Cheatsheet intake");
    let source = OpenRussianClient::from_share_url(&config.openrussian_url)?;
    let spanish_translation_source = SpanishTranslationProvider::new()?;

    if dry_run {
        let word_snapshot = source.load_snapshot()?;
        let russian_words = word_snapshot
            .words
            .iter()
            .filter(|word| !word.disabled)
            .map(|word| word.russian.clone())
            .collect::<Vec<_>>();
        let spanish_translations =
            spanish_translation_source.find_spanish_translations(&russian_words)?;
        let relation_snapshot = source.load_word_relation_snapshot()?;
        let example_snapshot = source.load_example_snapshot()?;
        let desired_state = desired_state::build(
            word_snapshot,
            relation_snapshot,
            example_snapshot,
            &spanish_translations,
        )?;
        let output_path = desired_state_file::default_path();
        desired_state_file::write(&output_path, &desired_state)?;
        if desired_state.summary.errors > 0 {
            tracing::warn!(
                errors = desired_state.summary.errors,
                path = %output_path.display(),
                "Desired state contains entries that could not be prepared; inspect the errors field"
            );
        }
        tracing::info!(path = %output_path.display(), "Desired state written");
        return Ok(RunSummary::DryRun(desired_state.summary));
    }

    let api_config = config
        .api
        .context("API configuration is required outside dry-run mode")?;
    let api = connect_api(api_config)?;
    let relation_snapshot = source.load_word_relation_snapshot()?;
    let example_snapshot = source.load_example_snapshot()?;
    let batch_size = config.batch_size;
    let word_synchronizer = SynchronizeWords::new(source, api.clone(), batch_size)
        .with_spanish_translation_source(spanish_translation_source);
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
    Ok(RunSummary::Sync(IntakeSummary {
        words: word_result.summary,
        relations,
        examples,
    }))
}

fn connect_api(config: ApiConfig) -> Result<CheatsheetApi> {
    let keycloak = Arc::new(JwtKeycloakContextHolder::new(
        &config.keycloak_token_url,
        config.keycloak_client_id,
        config.keycloak_client_secret,
    )?);
    keycloak.get_token()?;
    Ok(CheatsheetApi::new(&config.api_base_url, keycloak)?)
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
        words_with_spanish_translation = summary.words.spanish_translated,
        words_spanish_translation_pending = summary.words.spanish_pending,
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

fn log_desired_state_summary(summary: &DesiredStateSummary) {
    tracing::info!(
        words_scanned = summary.words_scanned,
        words_included = summary.words_included,
        words_disabled_skipped = summary.words_disabled_skipped,
        words_with_spanish_translation = summary.words_spanish_translated,
        words_spanish_translation_pending = summary.words_spanish_pending,
        relations_scanned = summary.relations_scanned,
        relations_included = summary.relations_included,
        examples_scanned = summary.examples_scanned,
        examples_included = summary.examples_included,
        examples_disabled_skipped = summary.examples_disabled_skipped,
        examples_unlinked_skipped = summary.examples_unlinked_skipped,
        errors = summary.errors,
        "Dry-run desired state complete"
    );
}
