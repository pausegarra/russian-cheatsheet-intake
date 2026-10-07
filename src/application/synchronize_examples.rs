use crate::application::ports::{
    ApiExample, ApiWord, ApiWriteError, ExampleWriteResult, ExamplesApi, SourceExampleSnapshot,
    WriteEffect,
};
use crate::domain::example::{ExampleTranslation, ImportedExample};
use anyhow::Result;
use std::collections::HashMap;
use tracing::{error, info, warn};

#[derive(Default)]
pub struct ExampleSyncSummary {
    pub scanned: usize,
    pub disabled_skipped: usize,
    pub unlinked_skipped: usize,
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub failed: usize,
    pub fatal_error: Option<String>,
}

pub struct SynchronizeExamples<A> {
    api: A,
}

impl<A: ExamplesApi> SynchronizeExamples<A> {
    pub fn new(api: A) -> Self {
        Self { api }
    }

    pub fn run(
        &self,
        snapshot: SourceExampleSnapshot,
        words_by_external_id: &HashMap<String, ApiWord>,
    ) -> Result<ExampleSyncSummary> {
        let SourceExampleSnapshot {
            examples,
            mut translations_by_example_id,
            mut word_ids_by_example_id,
        } = snapshot;
        let mut summary = ExampleSyncSummary {
            scanned: examples.len(),
            ..ExampleSyncSummary::default()
        };

        info!(
            source_examples = summary.scanned,
            "Preparing OpenRussian examples"
        );
        let mut existing = self.api.find_all_examples_by_external_id()?;
        info!(
            api_examples = existing.len(),
            "Russian Cheatsheet examples loaded"
        );

        for source_example in examples {
            if source_example.disabled {
                summary.disabled_skipped += 1;
                info!(
                    external_id = %source_example.id,
                    russian = %source_example.russian,
                    "Disabled OpenRussian example skipped"
                );
                continue;
            }

            let linked_word_ids = word_ids_by_example_id
                .remove(&source_example.id)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|word_id| {
                    words_by_external_id
                        .get(&word_id)
                        .map(|word| word.id.clone())
                })
                .collect::<Vec<_>>();
            if linked_word_ids.is_empty() {
                summary.unlinked_skipped += 1;
                info!(
                    external_id = %source_example.id,
                    russian = %source_example.russian,
                    "OpenRussian example without linked words skipped"
                );
                continue;
            }

            let translations = translations_by_example_id
                .remove(&source_example.id)
                .unwrap_or_default()
                .into_iter()
                .map(|translation| ExampleTranslation {
                    language: translation.language,
                    text: translation.text,
                    position: translation.position,
                })
                .collect();
            let example = ImportedExample::new(
                source_example.id.clone(),
                source_example.russian.clone(),
                translations,
                None,
                None,
                linked_word_ids,
            )?;

            match existing.get(&example.external_id) {
                Some(api_example)
                    if api_example.checksum.as_deref() == Some(example.checksum.as_str()) =>
                {
                    summary.unchanged += 1;
                    info!(
                        external_id = %example.external_id,
                        russian = %example.russian,
                        checksum = %example.checksum,
                        "Example checksum matches; skipping API write"
                    );
                    continue;
                }
                Some(api_example) => {
                    info!(
                        external_id = %example.external_id,
                        russian = %example.russian,
                        api_example_id = %api_example.id,
                        api_checksum = ?api_example.checksum,
                        expected_checksum = %example.checksum,
                        "Updating example in Russian Cheatsheet API"
                    );
                }
                None => {
                    info!(
                        external_id = %example.external_id,
                        russian = %example.russian,
                        expected_checksum = %example.checksum,
                        "Creating example in Russian Cheatsheet API"
                    );
                }
            }

            let write = match existing.get(&example.external_id) {
                Some(api_example) => self.update_with_reconciliation(api_example, &example),
                None => self.create_with_reconciliation(&example),
            };

            match write {
                Ok(result) => {
                    match result.effect {
                        WriteEffect::Created => {
                            summary.created += 1;
                            info!(
                                external_id = %example.external_id,
                                russian = %example.russian,
                                api_example_id = %result.example.id,
                                "Example created"
                            );
                        }
                        WriteEffect::Updated => {
                            summary.updated += 1;
                            info!(
                                external_id = %example.external_id,
                                russian = %example.russian,
                                api_example_id = %result.example.id,
                                "Example updated"
                            );
                        }
                    }
                    existing.insert(example.external_id.clone(), result.example);
                }
                Err(write_error) => {
                    error!(
                        external_id = %example.external_id,
                        russian = %example.russian,
                        error = %write_error.message,
                        "Example synchronization failed"
                    );
                    summary.failed += 1;
                    if write_error.fatal {
                        summary.fatal_error = Some(write_error.message);
                        break;
                    }
                }
            }
        }

        info!(
            scanned = summary.scanned,
            disabled_skipped = summary.disabled_skipped,
            unlinked_skipped = summary.unlinked_skipped,
            created = summary.created,
            updated = summary.updated,
            unchanged = summary.unchanged,
            failed = summary.failed,
            "OpenRussian example sync complete"
        );
        Ok(summary)
    }

    fn create_with_reconciliation(
        &self,
        example: &ImportedExample,
    ) -> std::result::Result<ExampleWriteResult, ApiWriteError> {
        match self.api.create_example(example) {
            Ok(result) => Ok(result),
            Err(error) if error.outcome_unknown => {
                warn!(
                    external_id = %example.external_id,
                    "Create outcome is unknown; reconciling by externalId"
                );
                let Some(existing) = self.reconcile(example)? else {
                    return Err(fatal_write_error(format!(
                        "{}; externalId={} not found after reconciliation",
                        error.message, example.external_id
                    )));
                };
                if existing.checksum.as_deref() == Some(example.checksum.as_str()) {
                    return Ok(ExampleWriteResult {
                        example: existing,
                        effect: WriteEffect::Created,
                    });
                }
                self.update_with_reconciliation(&existing, example)
            }
            Err(error) => Err(error),
        }
    }

    fn update_with_reconciliation(
        &self,
        existing: &ApiExample,
        example: &ImportedExample,
    ) -> std::result::Result<ExampleWriteResult, ApiWriteError> {
        match self.api.update_example(existing, example) {
            Ok(result) => Ok(result),
            Err(error) if error.outcome_unknown => {
                warn!(
                    external_id = %example.external_id,
                    api_example_id = %existing.id,
                    "Update outcome is unknown; reconciling by externalId"
                );
                let Some(current) = self.reconcile(example)? else {
                    return Err(fatal_write_error(format!(
                        "{}; externalId={} not found after reconciliation",
                        error.message, example.external_id
                    )));
                };
                if current.id == existing.id
                    && current.checksum.as_deref() == Some(example.checksum.as_str())
                {
                    return Ok(ExampleWriteResult {
                        example: current,
                        effect: WriteEffect::Updated,
                    });
                }
                if current.id != existing.id {
                    return Err(fatal_write_error(format!(
                        "{}; externalId={} changed API UUID from {} to {}",
                        error.message, example.external_id, existing.id, current.id
                    )));
                }

                warn!(
                    external_id = %example.external_id,
                    api_example_id = %current.id,
                    "Update checksum is still stale after reconciliation; retrying PUT once"
                );
                match self.api.update_example(&current, example) {
                    Ok(result) => Ok(result),
                    Err(retry_error) if retry_error.outcome_unknown => {
                        let Some(retried_current) = self.reconcile(example)? else {
                            return Err(fatal_write_error(format!(
                                "{}; retry failed: {}; externalId={} not found after reconciliation",
                                error.message, retry_error.message, example.external_id
                            )));
                        };
                        if retried_current.id == existing.id
                            && retried_current.checksum.as_deref()
                                == Some(example.checksum.as_str())
                        {
                            Ok(ExampleWriteResult {
                                example: retried_current,
                                effect: WriteEffect::Updated,
                            })
                        } else {
                            Err(fatal_write_error(format!(
                                "{}; retry failed: {}; externalId={} remains at API UUID {} with checksum {}",
                                error.message,
                                retry_error.message,
                                example.external_id,
                                retried_current.id,
                                retried_current.checksum.as_deref().unwrap_or("<null>")
                            )))
                        }
                    }
                    Err(retry_error) => Err(retry_error),
                }
            }
            Err(error) => Err(error),
        }
    }

    fn reconcile(
        &self,
        example: &ImportedExample,
    ) -> std::result::Result<Option<ApiExample>, ApiWriteError> {
        self.api
            .find_all_examples_by_external_id()
            .map(|mut examples| examples.remove(&example.external_id))
            .map_err(|error| {
                fatal_write_error(format!(
                    "could not reconcile externalId={}: {error:#}",
                    example.external_id
                ))
            })
    }
}

fn fatal_write_error(message: String) -> ApiWriteError {
    ApiWriteError {
        message,
        outcome_unknown: true,
        fatal: true,
    }
}
