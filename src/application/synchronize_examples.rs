use crate::application::ports::{
    ApiExample, ApiWord, ApiWriteError, ExamplesApi, SourceExampleSnapshot,
};
use crate::domain::example::{ExampleTranslation, ImportedExample};
use anyhow::Result;
use std::collections::HashMap;
use tracing::{error, info};

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
    batch_size: usize,
}

impl<A: ExamplesApi> SynchronizeExamples<A> {
    pub fn new(api: A, batch_size: usize) -> Self {
        Self { api, batch_size }
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

        let mut creates = Vec::new();
        let mut updates = Vec::new();
        let mut stop = false;

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

            match existing.get(&example.external_id) {
                Some(api_example) => updates.push((api_example.clone(), example)),
                None => creates.push(example),
            }

            if creates.len() == self.batch_size {
                let batch = std::mem::take(&mut creates);
                if self.flush_creates(&batch, &mut existing, &mut summary) {
                    summary.failed += updates.len();
                    updates.clear();
                    stop = true;
                    break;
                }
            }
            if updates.len() == self.batch_size {
                let batch = std::mem::take(&mut updates);
                if self.flush_updates(&batch, &mut existing, &mut summary) {
                    summary.failed += creates.len();
                    creates.clear();
                    stop = true;
                    break;
                }
            }
        }

        if !stop {
            let batch = std::mem::take(&mut creates);
            if !batch.is_empty() && self.flush_creates(&batch, &mut existing, &mut summary) {
                summary.failed += updates.len();
                updates.clear();
                stop = true;
            }
        }
        if !stop {
            let batch = std::mem::take(&mut updates);
            if !batch.is_empty() {
                self.flush_updates(&batch, &mut existing, &mut summary);
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

    fn flush_creates(
        &self,
        batch: &[ImportedExample],
        existing: &mut HashMap<String, ApiExample>,
        summary: &mut ExampleSyncSummary,
    ) -> bool {
        match self.api.create_examples_batch(batch) {
            Ok(results) => {
                if let Some(message) = validate_create_response(batch, &results) {
                    return self.reconcile_create_batch(
                        batch,
                        &ApiWriteError {
                            message,
                            outcome_unknown: true,
                            fatal: true,
                        },
                        existing,
                        summary,
                    );
                }
                for (example, api_example) in batch.iter().zip(results) {
                    info!(
                        external_id = %example.external_id,
                        russian = %example.russian,
                        api_example_id = %api_example.id,
                        "Example created"
                    );
                    summary.created += 1;
                    existing.insert(example.external_id.clone(), api_example);
                }
                false
            }
            Err(write_error) if write_error.outcome_unknown => {
                self.reconcile_create_batch(batch, &write_error, existing, summary)
            }
            Err(write_error) => {
                log_failed_examples(batch, &write_error.message);
                summary.failed += batch.len();
                if write_error.fatal {
                    summary.fatal_error = Some(write_error.message);
                }
                write_error.fatal
            }
        }
    }

    fn flush_updates(
        &self,
        batch: &[(ApiExample, ImportedExample)],
        existing: &mut HashMap<String, ApiExample>,
        summary: &mut ExampleSyncSummary,
    ) -> bool {
        match self.api.update_examples_batch(batch) {
            Ok(results) => {
                if let Some(message) = validate_update_response(batch, &results) {
                    return self.reconcile_update_batch(
                        batch,
                        &ApiWriteError {
                            message,
                            outcome_unknown: true,
                            fatal: true,
                        },
                        existing,
                        summary,
                    );
                }
                for ((_, example), api_example) in batch.iter().zip(results) {
                    info!(
                        external_id = %example.external_id,
                        russian = %example.russian,
                        api_example_id = %api_example.id,
                        "Example updated"
                    );
                    summary.updated += 1;
                    existing.insert(example.external_id.clone(), api_example);
                }
                false
            }
            Err(write_error) if write_error.outcome_unknown => {
                self.reconcile_update_batch(batch, &write_error, existing, summary)
            }
            Err(write_error) => {
                log_failed_example_updates(batch, &write_error.message);
                summary.failed += batch.len();
                if write_error.fatal {
                    summary.fatal_error = Some(write_error.message);
                }
                write_error.fatal
            }
        }
    }

    fn reconcile_create_batch(
        &self,
        batch: &[ImportedExample],
        write_error: &ApiWriteError,
        existing: &mut HashMap<String, ApiExample>,
        summary: &mut ExampleSyncSummary,
    ) -> bool {
        let mut refreshed = match self.api.find_all_examples_by_external_id() {
            Ok(examples) => examples,
            Err(error) => {
                log_failed_examples(batch, &write_error.message);
                summary.failed += batch.len();
                summary.fatal_error = Some(format!(
                    "{}; could not reconcile example batch: {error:#}",
                    write_error.message
                ));
                return true;
            }
        };

        let mut unresolved = Vec::new();
        for example in batch {
            match refreshed.get(&example.external_id) {
                Some(api_example)
                    if api_example.checksum.as_deref() == Some(example.checksum.as_str()) =>
                {
                    info!(
                        external_id = %example.external_id,
                        russian = %example.russian,
                        api_example_id = %api_example.id,
                        "Example create outcome reconciled"
                    );
                    summary.created += 1;
                }
                Some(api_example) => unresolved.push(format!(
                    "externalId={} has checksum {} instead of {}",
                    example.external_id,
                    api_example.checksum.as_deref().unwrap_or("<null>"),
                    example.checksum
                )),
                None => {
                    unresolved.push(format!("externalId={} was not found", example.external_id))
                }
            }
        }

        *existing = std::mem::take(&mut refreshed);
        if unresolved.is_empty() {
            false
        } else {
            for message in &unresolved {
                error!(error = %message, "Example create outcome unresolved after reconciliation");
            }
            summary.failed += unresolved.len();
            summary.fatal_error = Some(format!(
                "{}; example batch reconciliation unresolved: {}",
                write_error.message,
                unresolved.join("; ")
            ));
            true
        }
    }

    fn reconcile_update_batch(
        &self,
        batch: &[(ApiExample, ImportedExample)],
        write_error: &ApiWriteError,
        existing: &mut HashMap<String, ApiExample>,
        summary: &mut ExampleSyncSummary,
    ) -> bool {
        let mut refreshed = match self.api.find_all_examples_by_external_id() {
            Ok(examples) => examples,
            Err(error) => {
                log_failed_example_updates(batch, &write_error.message);
                summary.failed += batch.len();
                summary.fatal_error = Some(format!(
                    "{}; could not reconcile example batch: {error:#}",
                    write_error.message
                ));
                return true;
            }
        };

        let mut unresolved = Vec::new();
        for (original, example) in batch {
            match refreshed.get(&example.external_id) {
                Some(api_example)
                    if api_example.id == original.id
                        && api_example.checksum.as_deref() == Some(example.checksum.as_str()) =>
                {
                    info!(
                        external_id = %example.external_id,
                        russian = %example.russian,
                        api_example_id = %api_example.id,
                        "Example update outcome reconciled"
                    );
                    summary.updated += 1;
                }
                Some(api_example) if api_example.id != original.id => unresolved.push(format!(
                    "externalId={} changed API UUID from {} to {}",
                    example.external_id, original.id, api_example.id
                )),
                Some(api_example) => unresolved.push(format!(
                    "externalId={} has checksum {} instead of {}",
                    example.external_id,
                    api_example.checksum.as_deref().unwrap_or("<null>"),
                    example.checksum
                )),
                None => {
                    unresolved.push(format!("externalId={} was not found", example.external_id))
                }
            }
        }

        *existing = std::mem::take(&mut refreshed);
        if unresolved.is_empty() {
            false
        } else {
            for message in &unresolved {
                error!(error = %message, "Example update outcome unresolved after reconciliation");
            }
            summary.failed += unresolved.len();
            summary.fatal_error = Some(format!(
                "{}; example batch reconciliation unresolved: {}",
                write_error.message,
                unresolved.join("; ")
            ));
            true
        }
    }
}

fn validate_create_response(batch: &[ImportedExample], results: &[ApiExample]) -> Option<String> {
    if batch.len() != results.len() {
        return Some(format!(
            "API returned {} examples for create batch of {}",
            results.len(),
            batch.len()
        ));
    }
    for (example, api_example) in batch.iter().zip(results) {
        if api_example.external_id != example.external_id {
            return Some(format!(
                "API returned externalId={} for requested externalId={}",
                api_example.external_id, example.external_id
            ));
        }
        if api_example.checksum.as_deref() != Some(example.checksum.as_str()) {
            return Some(format!(
                "API checksum mismatch for example externalId={}: expected {}, received {}",
                example.external_id,
                example.checksum,
                api_example.checksum.as_deref().unwrap_or("<null>")
            ));
        }
    }
    None
}

fn validate_update_response(
    batch: &[(ApiExample, ImportedExample)],
    results: &[ApiExample],
) -> Option<String> {
    if batch.len() != results.len() {
        return Some(format!(
            "API returned {} examples for update batch of {}",
            results.len(),
            batch.len()
        ));
    }
    for ((original, example), api_example) in batch.iter().zip(results) {
        if api_example.external_id != example.external_id {
            return Some(format!(
                "API returned externalId={} for requested externalId={}",
                api_example.external_id, example.external_id
            ));
        }
        if api_example.id != original.id {
            return Some(format!(
                "API changed UUID for example externalId={}: expected {}, received {}",
                example.external_id, original.id, api_example.id
            ));
        }
        if api_example.checksum.as_deref() != Some(example.checksum.as_str()) {
            return Some(format!(
                "API checksum mismatch for example externalId={}: expected {}, received {}",
                example.external_id,
                example.checksum,
                api_example.checksum.as_deref().unwrap_or("<null>")
            ));
        }
    }
    None
}

fn log_failed_examples(batch: &[ImportedExample], message: &str) {
    for example in batch {
        error!(
            external_id = %example.external_id,
            russian = %example.russian,
            error = %message,
            "Example synchronization failed"
        );
    }
}

fn log_failed_example_updates(batch: &[(ApiExample, ImportedExample)], message: &str) {
    for (_, example) in batch {
        error!(
            external_id = %example.external_id,
            russian = %example.russian,
            error = %message,
            "Example synchronization failed"
        );
    }
}
