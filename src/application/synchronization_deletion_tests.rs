use super::ports::{
    ApiExample, ApiWord, ApiWordRelation, ApiWordRelationCreate, ApiWordRelationResult,
    ApiWriteError, ExamplesApi, SourceExample, SourceExampleSnapshot, SourceForm, SourceSnapshot,
    SourceWord, SourceWordRelation, SourceWordRelationSnapshot, WordRelationsApi, WordSource,
    WordsApi,
};
use super::synchronize::{SyncSummary, SynchronizeWords};
use super::synchronize_examples::SynchronizeExamples;
use super::synchronize_word_relations::SynchronizeWordRelations;
use crate::domain::example::ImportedExample;
use crate::domain::word::ImportedWord;
use anyhow::Result;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

struct FakeWordSource(RefCell<Option<SourceSnapshot>>);

impl WordSource for FakeWordSource {
    fn load_snapshot(&self) -> Result<SourceSnapshot> {
        Ok(self.0.borrow_mut().take().expect("snapshot is loaded once"))
    }
}

struct FakeWordsApi {
    words: RefCell<HashMap<String, ApiWord>>,
    deleted: RefCell<Vec<ApiWord>>,
    unknown_delete_outcome: bool,
}

impl WordsApi for FakeWordsApi {
    fn find_all_by_external_id(&self) -> Result<HashMap<String, ApiWord>> {
        Ok(self.words.borrow().clone())
    }

    fn create_batch(
        &self,
        _words: &[ImportedWord],
    ) -> std::result::Result<Vec<ApiWord>, ApiWriteError> {
        unreachable!("test snapshot has no valid words to create")
    }

    fn update_batch(
        &self,
        _updates: &[(ApiWord, ImportedWord)],
    ) -> std::result::Result<Vec<ApiWord>, ApiWriteError> {
        unreachable!("test snapshot has no valid words to update")
    }

    fn delete_words_batch(&self, words: &[ApiWord]) -> std::result::Result<(), ApiWriteError> {
        self.deleted.borrow_mut().extend_from_slice(words);
        let ids = words
            .iter()
            .map(|word| word.external_id.as_str())
            .collect::<HashSet<_>>();
        self.words
            .borrow_mut()
            .retain(|external_id, _| !ids.contains(external_id.as_str()));
        if self.unknown_delete_outcome {
            Err(ApiWriteError {
                message: "test timeout after word delete".to_owned(),
                outcome_unknown: true,
                fatal: false,
            })
        } else {
            Ok(())
        }
    }
}

struct FakeExamplesApi {
    examples: RefCell<HashMap<String, ApiExample>>,
    deleted: RefCell<Vec<ApiExample>>,
    unknown_delete_outcome: bool,
}

impl ExamplesApi for FakeExamplesApi {
    fn find_all_examples_by_external_id(&self) -> Result<HashMap<String, ApiExample>> {
        Ok(self.examples.borrow().clone())
    }

    fn create_examples_batch(
        &self,
        _examples: &[ImportedExample],
    ) -> std::result::Result<Vec<ApiExample>, ApiWriteError> {
        unreachable!("test examples have no API-linked words")
    }

    fn update_examples_batch(
        &self,
        _updates: &[(ApiExample, ImportedExample)],
    ) -> std::result::Result<Vec<ApiExample>, ApiWriteError> {
        unreachable!("test examples have no API-linked words")
    }

    fn delete_examples_batch(
        &self,
        examples: &[ApiExample],
    ) -> std::result::Result<(), ApiWriteError> {
        self.deleted.borrow_mut().extend_from_slice(examples);
        let ids = examples
            .iter()
            .map(|example| example.external_id.as_str())
            .collect::<HashSet<_>>();
        self.examples
            .borrow_mut()
            .retain(|external_id, _| !ids.contains(external_id.as_str()));
        if self.unknown_delete_outcome {
            Err(ApiWriteError {
                message: "test timeout after example delete".to_owned(),
                outcome_unknown: true,
                fatal: false,
            })
        } else {
            Ok(())
        }
    }
}

struct FakeWordRelationsApi {
    relations: RefCell<Vec<ApiWordRelation>>,
    deleted: RefCell<Vec<ApiWordRelation>>,
    unknown_delete_outcome: bool,
}

impl WordRelationsApi for FakeWordRelationsApi {
    fn find_all_word_relations(&self) -> Result<Vec<ApiWordRelation>> {
        Ok(self.relations.borrow().clone())
    }

    fn create_word_relations_batch(
        &self,
        _relations: &[ApiWordRelationCreate],
    ) -> std::result::Result<Vec<ApiWordRelationResult>, ApiWriteError> {
        unreachable!("the active relation has no API word mapping")
    }

    fn delete_word_relations_batch(
        &self,
        relations: &[ApiWordRelation],
    ) -> std::result::Result<(), ApiWriteError> {
        self.deleted.borrow_mut().extend_from_slice(relations);
        let deleted = relations
            .iter()
            .map(ApiWordRelation::api_identity)
            .collect::<HashSet<_>>();
        self.relations
            .borrow_mut()
            .retain(|relation| !deleted.contains(&relation.api_identity()));
        if self.unknown_delete_outcome {
            Err(ApiWriteError {
                message: "test timeout after delete".to_owned(),
                outcome_unknown: true,
                fatal: false,
            })
        } else {
            Ok(())
        }
    }
}

#[test]
fn stale_words_exclude_disabled_and_preserve_active_invalid_source_words() {
    let active_external_id = "active-invalid".to_owned();
    let disabled_external_id = "disabled".to_owned();
    let snapshot = SourceSnapshot {
        words: vec![
            SourceWord {
                id: active_external_id.clone(),
                russian: "активный".to_owned(),
                source_type: None,
                disabled: false,
                audio_url: None,
                usage: None,
            },
            SourceWord {
                id: disabled_external_id.clone(),
                russian: "отключённый".to_owned(),
                source_type: Some("noun".to_owned()),
                disabled: true,
                audio_url: None,
                usage: None,
            },
        ],
        translations_by_word_id: HashMap::new(),
        aspects_by_word_id: HashMap::new(),
        forms_by_word_id: HashMap::from([(
            active_external_id.clone(),
            vec![SourceForm {
                id: 1,
                position: Some(0),
                form_type: "ru_base".to_owned(),
                form_bare: "активный".to_owned(),
            }],
        )]),
    };
    let api = FakeWordsApi {
        words: RefCell::new(HashMap::from([
            (
                active_external_id.clone(),
                ApiWord {
                    id: "api-active".to_owned(),
                    external_id: active_external_id.clone(),
                    checksum: Some("old-checksum".to_owned()),
                },
            ),
            (
                disabled_external_id.clone(),
                ApiWord {
                    id: "api-disabled".to_owned(),
                    external_id: disabled_external_id.clone(),
                    checksum: Some("old-checksum".to_owned()),
                },
            ),
        ])),
        deleted: RefCell::new(Vec::new()),
        unknown_delete_outcome: false,
    };

    let result = SynchronizeWords::new(FakeWordSource(RefCell::new(Some(snapshot))), api, 10)
        .run_with_words()
        .expect("word snapshot sync completes");

    assert_eq!(
        result.active_source_word_ids,
        HashSet::from([active_external_id.clone()])
    );
    assert_eq!(result.words_by_external_id.len(), 1);
    assert!(
        result
            .words_by_external_id
            .contains_key(&active_external_id)
    );
    assert_eq!(result.stale_words.len(), 1);
    assert_eq!(result.stale_words[0].external_id, disabled_external_id);
    assert_eq!(result.summary.failed, 1);
}

#[test]
fn unknown_word_delete_outcome_is_reconciled_by_external_id_and_uuid() {
    let stale = ApiWord {
        id: "api-stale".to_owned(),
        external_id: "stale-source".to_owned(),
        checksum: Some("checksum".to_owned()),
    };
    let api = FakeWordsApi {
        words: RefCell::new(HashMap::from([(stale.external_id.clone(), stale.clone())])),
        deleted: RefCell::new(Vec::new()),
        unknown_delete_outcome: true,
    };
    let synchronizer = SynchronizeWords::new(FakeWordSource(RefCell::new(None)), api, 10);
    let mut summary = SyncSummary::default();

    synchronizer.delete_stale_words(&[stale], &mut summary);

    assert_eq!(summary.deleted, 1);
    assert_eq!(summary.failed, 0);
    assert!(summary.fatal_error.is_none());
}

#[test]
fn active_unlinked_examples_are_protected_while_disabled_examples_are_deleted() {
    let disabled_id = "disabled-example".to_owned();
    let unlinked_id = "active-unlinked-example".to_owned();
    let api = FakeExamplesApi {
        examples: RefCell::new(HashMap::from([
            (
                disabled_id.clone(),
                ApiExample {
                    id: "api-disabled-example".to_owned(),
                    external_id: disabled_id.clone(),
                    checksum: None,
                },
            ),
            (
                unlinked_id.clone(),
                ApiExample {
                    id: "api-active-example".to_owned(),
                    external_id: unlinked_id.clone(),
                    checksum: None,
                },
            ),
        ])),
        deleted: RefCell::new(Vec::new()),
        unknown_delete_outcome: true,
    };
    let snapshot = SourceExampleSnapshot {
        examples: vec![
            SourceExample {
                id: disabled_id.clone(),
                russian: "отключено".to_owned(),
                disabled: true,
            },
            SourceExample {
                id: unlinked_id.clone(),
                russian: "без слова".to_owned(),
                disabled: false,
            },
        ],
        translations_by_example_id: HashMap::new(),
        word_ids_by_example_id: HashMap::new(),
    };

    let summary = SynchronizeExamples::new(api, 10)
        .run(snapshot, &HashMap::new())
        .expect("example snapshot sync completes");

    assert_eq!(summary.deleted, 1);
    assert_eq!(summary.disabled_skipped, 1);
    assert_eq!(summary.unlinked_skipped, 1);
}

#[test]
fn relation_sweep_deletes_missing_and_unidentifiable_rows_but_protects_active_keys() {
    let active_relation = ApiWordRelation {
        source_word_id: "api-source".to_owned(),
        source_external_id: Some("source".to_owned()),
        related_word_id: "api-target".to_owned(),
        related_external_id: Some("target".to_owned()),
        relation: "synonym".to_owned(),
    };
    let missing_relation = ApiWordRelation {
        source_word_id: "api-old-source".to_owned(),
        source_external_id: Some("old-source".to_owned()),
        related_word_id: "api-old-target".to_owned(),
        related_external_id: Some("old-target".to_owned()),
        relation: "related".to_owned(),
    };
    let manual_relation = ApiWordRelation {
        source_word_id: "api-manual".to_owned(),
        source_external_id: None,
        related_word_id: "api-target".to_owned(),
        related_external_id: Some("target".to_owned()),
        relation: "antonym".to_owned(),
    };
    let api = FakeWordRelationsApi {
        relations: RefCell::new(vec![active_relation, missing_relation, manual_relation]),
        deleted: RefCell::new(Vec::new()),
        unknown_delete_outcome: true,
    };
    let source_relation = SourceWordRelation {
        word_id: "source".to_owned(),
        related_word_id: "target".to_owned(),
        relation: "synonym".to_owned(),
    };
    let snapshot = SourceWordRelationSnapshot {
        relations: vec![source_relation],
    };
    let active_ids = HashSet::from(["source".to_owned(), "target".to_owned()]);

    let summary = SynchronizeWordRelations::new(api, 10)
        .run(snapshot, &HashMap::new(), &active_ids)
        .expect("relation snapshot sync completes");

    assert_eq!(summary.deleted, 2);
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.unmapped_skipped, 1);
}
