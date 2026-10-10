use crate::application::ports::{
    ApiWord, ApiWordRelation, ApiWordRelationCreate, ApiWriteError, SourceWordRelation,
    SourceWordRelationSnapshot, WordRelationKey, WordRelationsApi,
};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use tracing::{error, info, warn};

#[derive(Default)]
pub struct WordRelationSyncSummary {
    pub scanned: usize,
    pub candidates: usize,
    pub self_relations_skipped: usize,
    pub unmapped_skipped: usize,
    pub created: usize,
    pub unchanged: usize,
    pub deleted: usize,
    pub failed: usize,
    pub fatal_error: Option<String>,
}

pub struct SynchronizeWordRelations<A> {
    api: A,
    batch_size: usize,
}

impl<A: WordRelationsApi> SynchronizeWordRelations<A> {
    pub fn new(api: A, batch_size: usize) -> Self {
        Self { api, batch_size }
    }

    pub fn run(
        &self,
        snapshot: SourceWordRelationSnapshot,
        words_by_external_id: &HashMap<String, ApiWord>,
        active_source_word_ids: &HashSet<String>,
    ) -> Result<WordRelationSyncSummary> {
        let mut summary = WordRelationSyncSummary {
            scanned: snapshot.relations.len(),
            ..WordRelationSyncSummary::default()
        };
        let desired_relations = desired_relation_keys(&snapshot.relations, active_source_word_ids);
        let candidates = prepare_candidates(snapshot.relations, words_by_external_id, &mut summary);
        summary.candidates = candidates.len();

        info!("Loading existing Russian Cheatsheet word relations");
        let existing_rows = self.api.find_all_word_relations()?;
        info!(
            api_word_relations = existing_rows.len(),
            "Word relations loaded"
        );
        let mut existing = existing_rows
            .iter()
            .filter_map(ApiWordRelation::source_key)
            .collect::<HashSet<_>>();
        let mut deletion_candidates = HashMap::new();
        let mut existing_by_source_key = HashMap::new();
        for relation in existing_rows {
            let identity = relation.api_identity();
            if let Some(key) = relation.source_key() {
                existing_by_source_key.insert(key, identity.clone());
            }
            deletion_candidates.insert(identity, relation);
        }
        for key in desired_relations {
            if let Some(identity) = existing_by_source_key.remove(&key) {
                deletion_candidates.remove(&identity);
            }
        }
        let stale_relations = deletion_candidates.into_values().collect::<Vec<_>>();

        let mut creates = Vec::new();
        for candidate in candidates {
            if existing.contains(&candidate.key) {
                summary.unchanged += 1;
            } else {
                creates.push(candidate);
            }
        }

        let batch_size = self.batch_size.max(1);
        let mut start = 0;
        while start < creates.len() {
            let end = (start + batch_size).min(creates.len());
            if self.flush_create_batch(&creates[start..end], &mut existing, &mut summary) {
                let not_attempted = creates.len() - end;
                if not_attempted > 0 {
                    warn!(
                        not_attempted,
                        "Remaining word relation creates skipped after fatal batch failure"
                    );
                    summary.failed += not_attempted;
                }
                break;
            }
            start = end;
        }

        if summary.fatal_error.is_none() {
            self.delete_stale_relations(&stale_relations, &mut summary);
        }

        log_summary(&summary);
        Ok(summary)
    }

    fn delete_stale_relations(
        &self,
        stale_relations: &[ApiWordRelation],
        summary: &mut WordRelationSyncSummary,
    ) {
        let batch_size = self.batch_size.max(1);
        let mut start = 0;
        while start < stale_relations.len() {
            let end = (start + batch_size).min(stale_relations.len());
            let batch = &stale_relations[start..end];
            match self.api.delete_word_relations_batch(batch) {
                Ok(()) => {
                    summary.deleted += batch.len();
                    info!(relations = batch.len(), "Stale word relations deleted");
                    start = end;
                }
                Err(write_error) if write_error.outcome_unknown => {
                    match self.api.find_all_word_relations() {
                        Ok(refreshed) => {
                            let unresolved = batch
                                .iter()
                                .filter(|requested| {
                                    refreshed.iter().any(|current| {
                                        current.api_identity() == requested.api_identity()
                                    })
                                })
                                .collect::<Vec<_>>();
                            summary.deleted += batch.len() - unresolved.len();
                            if unresolved.is_empty() {
                                info!(
                                    relations = batch.len(),
                                    "Stale word relation delete outcome reconciled"
                                );
                                start = end;
                            } else {
                                let not_attempted = stale_relations.len() - end;
                                summary.failed += unresolved.len() + not_attempted;
                                summary.fatal_error = Some(format!(
                                    "{}; {} stale word relations remain after reconciliation",
                                    write_error.message,
                                    unresolved.len()
                                ));
                                error!(
                                    unresolved_relations = unresolved.len(),
                                    not_attempted, "Stale word relation deletes unresolved"
                                );
                                break;
                            }
                        }
                        Err(error) => {
                            let not_attempted = stale_relations.len() - end;
                            summary.failed += batch.len() + not_attempted;
                            summary.fatal_error = Some(format!(
                                "{}; could not reconcile relation deletes: {error:#}",
                                write_error.message
                            ));
                            break;
                        }
                    }
                }
                Err(write_error) => {
                    let not_attempted = stale_relations.len() - end;
                    summary.failed += batch.len() + not_attempted;
                    summary.fatal_error = Some(write_error.message.clone());
                    error!(
                        relations = batch.len(),
                        not_attempted,
                        error = %write_error.message,
                        "Stale word relation delete failed"
                    );
                    break;
                }
            }
        }
    }

    fn flush_create_batch(
        &self,
        batch: &[ApiWordRelationCreate],
        existing: &mut HashSet<WordRelationKey>,
        summary: &mut WordRelationSyncSummary,
    ) -> bool {
        match self.api.create_word_relations_batch(batch) {
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

                for result in results {
                    if result.created {
                        summary.created += 1;
                        info!(
                            source_external_id = %result.key.source_external_id,
                            related_external_id = %result.key.related_external_id,
                            relation = %result.key.relation,
                            "Word relation created"
                        );
                    } else {
                        summary.unchanged += 1;
                        info!(
                            source_external_id = %result.key.source_external_id,
                            related_external_id = %result.key.related_external_id,
                            relation = %result.key.relation,
                            "Word relation already existed"
                        );
                    }
                    existing.insert(result.key);
                }
                false
            }
            Err(write_error) if write_error.outcome_unknown => {
                self.reconcile_create_batch(batch, &write_error, existing, summary)
            }
            Err(write_error) => {
                log_failed_batch(batch, &write_error.message);
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
        batch: &[ApiWordRelationCreate],
        write_error: &ApiWriteError,
        existing: &mut HashSet<WordRelationKey>,
        summary: &mut WordRelationSyncSummary,
    ) -> bool {
        let refreshed = match self.api.find_all_word_relations() {
            Ok(relations) => relations,
            Err(error) => {
                log_failed_batch(batch, &write_error.message);
                summary.failed += batch.len();
                summary.fatal_error = Some(format!(
                    "{}; could not reconcile word relation batch: {error:#}",
                    write_error.message
                ));
                return true;
            }
        };

        let mut refreshed_keys = refreshed
            .iter()
            .filter_map(ApiWordRelation::source_key)
            .collect::<HashSet<_>>();
        let unresolved = batch
            .iter()
            .filter(|candidate| !refreshed_keys.contains(&candidate.key))
            .cloned()
            .collect::<Vec<_>>();
        let resolved = batch.len() - unresolved.len();
        summary.created += resolved;
        for candidate in batch {
            if refreshed_keys.contains(&candidate.key) {
                info!(
                    source_external_id = %candidate.key.source_external_id,
                    related_external_id = %candidate.key.related_external_id,
                    relation = %candidate.key.relation,
                    "Word relation create outcome reconciled"
                );
            }
        }

        *existing = std::mem::take(&mut refreshed_keys);
        if unresolved.is_empty() {
            false
        } else {
            error!(
                unresolved_relations = unresolved.len(),
                batch_size = batch.len(),
                error = %write_error.message,
                "Word relation batch outcome unresolved after reconciliation"
            );
            summary.failed += unresolved.len();
            summary.fatal_error = Some(format!(
                "{}; {} word relations unresolved after reconciliation",
                write_error.message,
                unresolved.len()
            ));
            true
        }
    }
}

fn prepare_candidates(
    relations: Vec<SourceWordRelation>,
    words_by_external_id: &HashMap<String, ApiWord>,
    summary: &mut WordRelationSyncSummary,
) -> Vec<ApiWordRelationCreate> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    for relation in relations {
        if relation.word_id == relation.related_word_id {
            summary.self_relations_skipped += 1;
            info!(
                external_id = %relation.word_id,
                relation = %relation.relation,
                "OpenRussian self relation skipped by external ID"
            );
            continue;
        }

        if !matches!(
            relation.relation.as_str(),
            "related" | "synonym" | "antonym"
        ) {
            error!(
                word_id = %relation.word_id,
                related_word_id = %relation.related_word_id,
                relation = %relation.relation,
                "Unsupported OpenRussian word relation"
            );
            summary.failed += 1;
            continue;
        }

        let (Some(word), Some(related_word)) = (
            words_by_external_id.get(&relation.word_id),
            words_by_external_id.get(&relation.related_word_id),
        ) else {
            summary.unmapped_skipped += 1;
            info!(
                word_id = %relation.word_id,
                related_word_id = %relation.related_word_id,
                "Word relation skipped because one or both API words are unavailable"
            );
            continue;
        };

        if word.external_id == related_word.external_id {
            summary.self_relations_skipped += 1;
            info!(
                external_id = %word.external_id,
                relation = %relation.relation,
                "OpenRussian self relation skipped by API external ID"
            );
            continue;
        }
        if word.id == related_word.id {
            error!(
                source_external_id = %word.external_id,
                related_external_id = %related_word.external_id,
                api_word_id = %word.id,
                "Different external IDs resolve to the same API word"
            );
            summary.failed += 1;
            continue;
        }

        let forward = ApiWordRelationCreate {
            key: WordRelationKey {
                source_external_id: relation.word_id.clone(),
                related_external_id: relation.related_word_id.clone(),
                relation: relation.relation.clone(),
            },
            word_id: word.id.clone(),
            related_word_id: related_word.id.clone(),
        };
        let inverse = ApiWordRelationCreate {
            key: WordRelationKey {
                source_external_id: relation.related_word_id,
                related_external_id: relation.word_id,
                relation: relation.relation,
            },
            word_id: related_word.id.clone(),
            related_word_id: word.id.clone(),
        };
        for candidate in [forward, inverse] {
            if seen.insert(candidate.key.clone()) {
                candidates.push(candidate);
            }
        }
    }

    candidates
}

fn desired_relation_keys(
    relations: &[SourceWordRelation],
    active_source_word_ids: &HashSet<String>,
) -> HashSet<WordRelationKey> {
    let mut desired = HashSet::new();
    for relation in relations {
        if relation.word_id == relation.related_word_id
            || !matches!(
                relation.relation.as_str(),
                "related" | "synonym" | "antonym"
            )
            || !active_source_word_ids.contains(&relation.word_id)
            || !active_source_word_ids.contains(&relation.related_word_id)
        {
            continue;
        }

        desired.insert(WordRelationKey {
            source_external_id: relation.word_id.clone(),
            related_external_id: relation.related_word_id.clone(),
            relation: relation.relation.clone(),
        });
        desired.insert(WordRelationKey {
            source_external_id: relation.related_word_id.clone(),
            related_external_id: relation.word_id.clone(),
            relation: relation.relation.clone(),
        });
    }
    desired
}

fn validate_create_response(
    batch: &[ApiWordRelationCreate],
    results: &[crate::application::ports::ApiWordRelationResult],
) -> Option<String> {
    if batch.len() != results.len() {
        return Some(format!(
            "API returned {} word relations for create batch of {}",
            results.len(),
            batch.len()
        ));
    }

    for (expected, result) in batch.iter().zip(results) {
        if expected.key != result.key {
            return Some(format!(
                "API returned word relation {} -> {} ({}) for requested {} -> {} ({})",
                result.key.source_external_id,
                result.key.related_external_id,
                result.key.relation,
                expected.key.source_external_id,
                expected.key.related_external_id,
                expected.key.relation
            ));
        }
    }

    None
}

fn log_failed_batch(batch: &[ApiWordRelationCreate], message: &str) {
    error!(
        relations = batch.len(),
        error = %message,
        "Word relation batch failed or could not be reconciled"
    );
}

fn log_summary(summary: &WordRelationSyncSummary) {
    info!(
        scanned = summary.scanned,
        candidates = summary.candidates,
        self_relations_skipped = summary.self_relations_skipped,
        unmapped_skipped = summary.unmapped_skipped,
        created = summary.created,
        unchanged = summary.unchanged,
        deleted = summary.deleted,
        failed = summary.failed,
        "OpenRussian word relation sync complete"
    );
    if let Some(error) = &summary.fatal_error {
        error!(error = %error, "Word relation intake stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::{WordRelationSyncSummary, desired_relation_keys, prepare_candidates};
    use crate::application::ports::{ApiWord, SourceWordRelation, WordRelationKey};
    use std::collections::{HashMap, HashSet};

    #[test]
    fn skips_source_relations_that_point_to_the_same_external_word() {
        let external_id = "same-openrussian-id".to_owned();
        let words_by_external_id = HashMap::from([(
            external_id.clone(),
            ApiWord {
                id: "api-word-uuid".to_owned(),
                external_id: external_id.clone(),
                checksum: None,
            },
        )]);
        let relation = SourceWordRelation {
            word_id: external_id.clone(),
            related_word_id: external_id,
            relation: "related".to_owned(),
        };
        let mut summary = WordRelationSyncSummary::default();

        let candidates = prepare_candidates(vec![relation], &words_by_external_id, &mut summary);

        assert!(candidates.is_empty());
        assert_eq!(summary.self_relations_skipped, 1);
    }

    #[test]
    fn relation_identity_uses_external_ids_not_api_word_uuids() {
        let source_external_id = "source-external-id".to_owned();
        let related_external_id = "related-external-id".to_owned();
        let words_by_external_id = HashMap::from([
            (
                source_external_id.clone(),
                ApiWord {
                    id: "source-api-uuid".to_owned(),
                    external_id: source_external_id.clone(),
                    checksum: None,
                },
            ),
            (
                related_external_id.clone(),
                ApiWord {
                    id: "related-api-uuid".to_owned(),
                    external_id: related_external_id.clone(),
                    checksum: None,
                },
            ),
        ]);
        let relation = SourceWordRelation {
            word_id: source_external_id.clone(),
            related_word_id: related_external_id.clone(),
            relation: "synonym".to_owned(),
        };
        let mut summary = WordRelationSyncSummary::default();

        let candidates = prepare_candidates(vec![relation], &words_by_external_id, &mut summary);

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].key.source_external_id, source_external_id);
        assert_eq!(candidates[0].key.related_external_id, related_external_id);
        assert_eq!(candidates[1].key.source_external_id, "related-external-id");
        assert_eq!(candidates[1].key.related_external_id, "source-external-id");
    }

    #[test]
    fn desired_relations_include_both_directions_for_active_words_without_api_ids() {
        let relation = SourceWordRelation {
            word_id: "source".to_owned(),
            related_word_id: "target".to_owned(),
            relation: "synonym".to_owned(),
        };
        let active = HashSet::from(["source".to_owned(), "target".to_owned()]);

        let desired = desired_relation_keys(&[relation], &active);

        assert!(desired.contains(&WordRelationKey {
            source_external_id: "source".to_owned(),
            related_external_id: "target".to_owned(),
            relation: "synonym".to_owned(),
        }));
        assert!(desired.contains(&WordRelationKey {
            source_external_id: "target".to_owned(),
            related_external_id: "source".to_owned(),
            relation: "synonym".to_owned(),
        }));
    }
}
