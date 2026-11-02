use crate::application::ports::{
    SourceExampleSnapshot, SourceSnapshot, SourceWordRelationSnapshot,
};
use crate::application::synchronize::{
    PreparedWord, SyncSummary, prepare_words_with_spanish_translations,
};
use crate::application::synchronize_word_relations::desired_relation_keys;
use crate::domain::example::ExampleTranslation;
use anyhow::Result;
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesiredState {
    pub format_version: u8,
    pub words: Vec<Value>,
    pub relations: Vec<DesiredRelation>,
    pub examples: Vec<DesiredExample>,
    pub pending_translations: Vec<PendingTranslation>,
    pub errors: Vec<DesiredStateError>,
    pub summary: DesiredStateSummary,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesiredRelation {
    pub word_external_id: String,
    pub related_word_external_id: String,
    pub relation: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesiredExample {
    pub external_id: String,
    pub russian: String,
    pub translations: Vec<ExampleTranslation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contributor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_url: Option<String>,
    pub linked_word_external_ids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingTranslation {
    pub external_id: String,
    pub russian: String,
    pub languages: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesiredStateError {
    pub kind: String,
    pub external_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub russian: Option<String>,
    pub message: String,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesiredStateSummary {
    pub words_scanned: usize,
    pub words_included: usize,
    pub words_disabled_skipped: usize,
    pub words_spanish_translated: usize,
    pub words_spanish_pending: usize,
    pub relations_scanned: usize,
    pub relations_included: usize,
    pub examples_scanned: usize,
    pub examples_included: usize,
    pub examples_disabled_skipped: usize,
    pub examples_unlinked_skipped: usize,
    pub errors: usize,
}

pub fn build(
    word_snapshot: SourceSnapshot,
    relation_snapshot: SourceWordRelationSnapshot,
    example_snapshot: SourceExampleSnapshot,
    spanish_translations_by_russian: &HashMap<String, Vec<String>>,
) -> Result<DesiredState> {
    let mut preparation_summary = SyncSummary::default();
    preparation_summary.scanned = word_snapshot.words.len();
    let prepared = prepare_words_with_spanish_translations(
        word_snapshot,
        &mut preparation_summary,
        spanish_translations_by_russian,
    );
    let mut words = Vec::new();
    let mut valid_word_external_ids = HashSet::new();
    let mut pending_translations = Vec::new();
    let mut words_spanish_translated = 0;
    let mut errors = Vec::new();

    for item in prepared {
        match item {
            PreparedWord::Ready(word) => {
                valid_word_external_ids.insert(word.external_id.clone());
                if word
                    .translations
                    .iter()
                    .any(|translation| translation.language == "es")
                {
                    words_spanish_translated += 1;
                } else {
                    pending_translations.push(PendingTranslation {
                        external_id: word.external_id.clone(),
                        russian: word.russian.clone(),
                        languages: vec!["es".to_owned()],
                    });
                }
                let mut payload = serde_json::to_value(word.request())?;
                omit_null_properties(&mut payload);
                words.push(payload);
            }
            PreparedWord::Invalid {
                external_id,
                russian,
                message,
            } => errors.push(DesiredStateError {
                kind: "word".to_owned(),
                external_id,
                russian: Some(russian),
                message,
            }),
        }
    }

    let mut relations = Vec::new();
    for source_relation in &relation_snapshot.relations {
        if source_relation.word_id == source_relation.related_word_id {
            continue;
        }
        if !matches!(
            source_relation.relation.as_str(),
            "related" | "synonym" | "antonym"
        ) {
            errors.push(DesiredStateError {
                kind: "relation".to_owned(),
                external_id: source_relation.word_id.clone(),
                russian: None,
                message: format!(
                    "unsupported OpenRussian word relation {} -> {}: {}",
                    source_relation.word_id,
                    source_relation.related_word_id,
                    source_relation.relation,
                ),
            });
        }
    }
    let mut relation_keys =
        desired_relation_keys(&relation_snapshot.relations, &valid_word_external_ids)
            .into_iter()
            .collect::<Vec<_>>();
    relation_keys.sort_by(|left, right| {
        left.source_external_id
            .cmp(&right.source_external_id)
            .then_with(|| left.related_external_id.cmp(&right.related_external_id))
            .then_with(|| left.relation.cmp(&right.relation))
    });
    relations.extend(relation_keys.into_iter().map(|key| DesiredRelation {
        word_external_id: key.source_external_id,
        related_word_external_id: key.related_external_id,
        relation: key.relation,
    }));

    let examples_scanned = example_snapshot.examples.len();
    let mut examples = Vec::new();
    let mut examples_disabled_skipped = 0;
    let mut examples_unlinked_skipped = 0;
    let mut translations_by_example_id = example_snapshot.translations_by_example_id;
    let mut word_ids_by_example_id = example_snapshot.word_ids_by_example_id;
    for source_example in example_snapshot.examples {
        if source_example.disabled {
            examples_disabled_skipped += 1;
            continue;
        }

        let mut linked_word_ids = Vec::new();
        let mut seen_word_ids = HashSet::new();
        for external_id in word_ids_by_example_id
            .remove(&source_example.id)
            .unwrap_or_default()
        {
            if valid_word_external_ids.contains(&external_id)
                && seen_word_ids.insert(external_id.clone())
            {
                linked_word_ids.push(external_id);
            }
        }
        if linked_word_ids.is_empty() {
            examples_unlinked_skipped += 1;
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
        examples.push(DesiredExample {
            external_id: source_example.id,
            russian: source_example.russian.replace('\'', ""),
            translations,
            contributor: None,
            audio_url: None,
            linked_word_external_ids: linked_word_ids,
        });
    }

    let summary = DesiredStateSummary {
        words_scanned: preparation_summary.scanned,
        words_included: words.len(),
        words_disabled_skipped: preparation_summary.disabled_skipped,
        words_spanish_translated,
        words_spanish_pending: pending_translations.len(),
        relations_scanned: relation_snapshot.relations.len(),
        relations_included: relations.len(),
        examples_scanned,
        examples_included: examples.len(),
        examples_disabled_skipped,
        examples_unlinked_skipped,
        errors: errors.len(),
    };

    Ok(DesiredState {
        format_version: 1,
        words,
        relations,
        examples,
        pending_translations,
        errors,
        summary,
    })
}

fn omit_null_properties(value: &mut Value) {
    match value {
        Value::Object(properties) => {
            properties.retain(|_, value| !value.is_null());
            properties.values_mut().for_each(omit_null_properties);
        }
        Value::Array(items) => items.iter_mut().for_each(omit_null_properties),
        _ => {}
    }
}
