use crate::application::ports::{
    ApiWord, ApiWriteError, SourceForm, SourceSnapshot, SourceWord, WordSource, WordsApi,
};
use crate::domain::word::{ImportedWord, Translation, WORD_FORM_TYPES, WordForms};
use anyhow::Result;
use std::cmp::Ordering;
use std::collections::HashMap;
use tracing::{debug, error, info, warn};

pub struct SynchronizeWords<S, A> {
    source: S,
    api: A,
    batch_size: usize,
}

#[derive(Default)]
pub struct SyncSummary {
    pub scanned: usize,
    pub disabled_skipped: usize,
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub failed: usize,
    pub fatal_error: Option<String>,
}

pub struct WordSyncResult {
    pub summary: SyncSummary,
    pub words_by_external_id: HashMap<String, ApiWord>,
}

enum PreparedWord {
    Ready(ImportedWord),
    Invalid {
        external_id: String,
        message: String,
    },
}

impl<S: WordSource, A: WordsApi> SynchronizeWords<S, A> {
    pub fn new(source: S, api: A, batch_size: usize) -> Self {
        Self {
            source,
            api,
            batch_size,
        }
    }

    pub fn run_with_words(&self) -> Result<WordSyncResult> {
        info!("Loading OpenRussian words and related source rows");
        let snapshot = self.source.load_snapshot()?;
        let mut summary = SyncSummary {
            scanned: snapshot.words.len(),
            ..SyncSummary::default()
        };
        let english_translation_count = snapshot
            .translations_by_word_id
            .values()
            .map(Vec::len)
            .sum::<usize>();
        info!(
            source_words = summary.scanned,
            english_translations = english_translation_count,
            verb_aspects = snapshot.aspects_by_word_id.len(),
            "OpenRussian source data loaded"
        );

        info!("Calculating expected word checksums");
        let prepared = prepare_words(snapshot, &mut summary);
        info!(
            active_words = prepared.len(),
            disabled_skipped = summary.disabled_skipped,
            "OpenRussian words prepared"
        );

        info!("Loading existing Russian Cheatsheet word checksums");
        let mut existing = self.api.find_all_by_external_id()?;
        info!(
            api_words = existing.len(),
            "Russian Cheatsheet words loaded"
        );

        let mut creates = Vec::new();
        let mut updates = Vec::new();
        let mut stop = false;

        for item in prepared {
            let word = match item {
                PreparedWord::Ready(word) => word,
                PreparedWord::Invalid {
                    external_id,
                    message,
                } => {
                    warn!(
                        external_id,
                        error = %message,
                        "OpenRussian word could not be prepared"
                    );
                    summary.failed += 1;
                    continue;
                }
            };

            match existing.get(&word.external_id) {
                Some(api_word) if api_word.checksum.as_deref() == Some(word.checksum.as_str()) => {
                    info!(
                        external_id = %word.external_id,
                        russian = %word.russian,
                        checksum = %word.checksum,
                        "Word checksum matches; skipping API write"
                    );
                    summary.unchanged += 1;
                    continue;
                }
                Some(api_word) => {
                    info!(
                        external_id = %word.external_id,
                        russian = %word.russian,
                        api_word_id = %api_word.id,
                        api_checksum = ?api_word.checksum,
                        expected_checksum = %word.checksum,
                        "Updating word in Russian Cheatsheet API"
                    );
                    updates.push((api_word.clone(), word));
                }
                None => {
                    info!(
                        external_id = %word.external_id,
                        russian = %word.russian,
                        expected_checksum = %word.checksum,
                        "Creating word in Russian Cheatsheet API"
                    );
                    creates.push(word);
                }
            }

            if creates.len() >= self.batch_size {
                let batch = std::mem::take(&mut creates);
                if self.flush_creates(&batch, &mut existing, &mut summary) {
                    summary.failed += updates.len();
                    updates.clear();
                    stop = true;
                    break;
                }
            }
            if updates.len() >= self.batch_size {
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

        Ok(WordSyncResult {
            summary,
            words_by_external_id: existing,
        })
    }

    fn flush_creates(
        &self,
        batch: &[ImportedWord],
        existing: &mut HashMap<String, ApiWord>,
        summary: &mut SyncSummary,
    ) -> bool {
        match self.api.create_batch(batch) {
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
                for (word, api_word) in batch.iter().zip(results) {
                    info!(
                        external_id = %word.external_id,
                        russian = %word.russian,
                        api_word_id = %api_word.id,
                        "Word created"
                    );
                    summary.created += 1;
                    existing.insert(word.external_id.clone(), api_word);
                }
                false
            }
            Err(write_error) if write_error.outcome_unknown => {
                self.reconcile_create_batch(batch, &write_error, existing, summary)
            }
            Err(write_error) => {
                log_failed_words(batch, &write_error.message);
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
        batch: &[(ApiWord, ImportedWord)],
        existing: &mut HashMap<String, ApiWord>,
        summary: &mut SyncSummary,
    ) -> bool {
        match self.api.update_batch(batch) {
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
                for ((_, word), api_word) in batch.iter().zip(results) {
                    info!(
                        external_id = %word.external_id,
                        russian = %word.russian,
                        api_word_id = %api_word.id,
                        "Word updated"
                    );
                    summary.updated += 1;
                    existing.insert(word.external_id.clone(), api_word);
                }
                false
            }
            Err(write_error) if write_error.outcome_unknown => {
                self.reconcile_update_batch(batch, &write_error, existing, summary)
            }
            Err(write_error) => {
                log_failed_word_updates(batch, &write_error.message);
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
        batch: &[ImportedWord],
        write_error: &ApiWriteError,
        existing: &mut HashMap<String, ApiWord>,
        summary: &mut SyncSummary,
    ) -> bool {
        let mut refreshed = match self.api.find_all_by_external_id() {
            Ok(words) => words,
            Err(error) => {
                log_failed_words(batch, &write_error.message);
                summary.failed += batch.len();
                summary.fatal_error = Some(format!(
                    "{}; could not reconcile word batch: {error:#}",
                    write_error.message
                ));
                return true;
            }
        };

        let mut unresolved = Vec::new();
        for word in batch {
            match refreshed.get(&word.external_id) {
                Some(api_word) if api_word.checksum.as_deref() == Some(word.checksum.as_str()) => {
                    info!(
                        external_id = %word.external_id,
                        russian = %word.russian,
                        api_word_id = %api_word.id,
                        "Word create outcome reconciled"
                    );
                    summary.created += 1;
                }
                Some(api_word) => unresolved.push(format!(
                    "externalId={} has checksum {} instead of {}",
                    word.external_id,
                    api_word.checksum.as_deref().unwrap_or("<null>"),
                    word.checksum
                )),
                None => unresolved.push(format!("externalId={} was not found", word.external_id)),
            }
        }

        *existing = std::mem::take(&mut refreshed);
        if unresolved.is_empty() {
            false
        } else {
            for message in &unresolved {
                error!(error = %message, "Word create outcome unresolved after reconciliation");
            }
            summary.failed += unresolved.len();
            summary.fatal_error = Some(format!(
                "{}; word batch reconciliation unresolved: {}",
                write_error.message,
                unresolved.join("; ")
            ));
            true
        }
    }

    fn reconcile_update_batch(
        &self,
        batch: &[(ApiWord, ImportedWord)],
        write_error: &ApiWriteError,
        existing: &mut HashMap<String, ApiWord>,
        summary: &mut SyncSummary,
    ) -> bool {
        let mut refreshed = match self.api.find_all_by_external_id() {
            Ok(words) => words,
            Err(error) => {
                log_failed_word_updates(batch, &write_error.message);
                summary.failed += batch.len();
                summary.fatal_error = Some(format!(
                    "{}; could not reconcile word batch: {error:#}",
                    write_error.message
                ));
                return true;
            }
        };

        let mut unresolved = Vec::new();
        for (original, word) in batch {
            match refreshed.get(&word.external_id) {
                Some(api_word)
                    if api_word.id == original.id
                        && api_word.checksum.as_deref() == Some(word.checksum.as_str()) =>
                {
                    info!(
                        external_id = %word.external_id,
                        russian = %word.russian,
                        api_word_id = %api_word.id,
                        "Word update outcome reconciled"
                    );
                    summary.updated += 1;
                }
                Some(api_word) if api_word.id != original.id => unresolved.push(format!(
                    "externalId={} changed API ID from {} to {}",
                    word.external_id, original.id, api_word.id
                )),
                Some(api_word) => unresolved.push(format!(
                    "externalId={} has checksum {} instead of {}",
                    word.external_id,
                    api_word.checksum.as_deref().unwrap_or("<null>"),
                    word.checksum
                )),
                None => unresolved.push(format!("externalId={} was not found", word.external_id)),
            }
        }

        *existing = std::mem::take(&mut refreshed);
        if unresolved.is_empty() {
            false
        } else {
            for message in &unresolved {
                error!(error = %message, "Word update outcome unresolved after reconciliation");
            }
            summary.failed += unresolved.len();
            summary.fatal_error = Some(format!(
                "{}; word batch reconciliation unresolved: {}",
                write_error.message,
                unresolved.join("; ")
            ));
            true
        }
    }
}

fn validate_create_response(batch: &[ImportedWord], results: &[ApiWord]) -> Option<String> {
    if batch.len() != results.len() {
        return Some(format!(
            "API returned {} words for create batch of {}",
            results.len(),
            batch.len()
        ));
    }
    for (word, api_word) in batch.iter().zip(results) {
        if api_word.external_id != word.external_id {
            return Some(format!(
                "API returned externalId={} for requested externalId={}",
                api_word.external_id, word.external_id
            ));
        }
        if api_word.checksum.as_deref() != Some(word.checksum.as_str()) {
            return Some(format!(
                "API checksum mismatch for externalId={}: expected {}, received {}",
                word.external_id,
                word.checksum,
                api_word.checksum.as_deref().unwrap_or("<null>")
            ));
        }
    }
    None
}

fn validate_update_response(
    batch: &[(ApiWord, ImportedWord)],
    results: &[ApiWord],
) -> Option<String> {
    if batch.len() != results.len() {
        return Some(format!(
            "API returned {} words for update batch of {}",
            results.len(),
            batch.len()
        ));
    }
    for ((original, word), api_word) in batch.iter().zip(results) {
        if api_word.external_id != word.external_id {
            return Some(format!(
                "API returned externalId={} for requested externalId={}",
                api_word.external_id, word.external_id
            ));
        }
        if api_word.id != original.id {
            return Some(format!(
                "API changed ID for externalId={}: expected {}, received {}",
                word.external_id, original.id, api_word.id
            ));
        }
        if api_word.checksum.as_deref() != Some(word.checksum.as_str()) {
            return Some(format!(
                "API checksum mismatch for externalId={}: expected {}, received {}",
                word.external_id,
                word.checksum,
                api_word.checksum.as_deref().unwrap_or("<null>")
            ));
        }
    }
    None
}

fn log_failed_words(batch: &[ImportedWord], message: &str) {
    for word in batch {
        error!(
            external_id = %word.external_id,
            russian = %word.russian,
            error = %message,
            "Word synchronization failed"
        );
    }
}

fn log_failed_word_updates(batch: &[(ApiWord, ImportedWord)], message: &str) {
    for (_, word) in batch {
        error!(
            external_id = %word.external_id,
            russian = %word.russian,
            error = %message,
            "Word synchronization failed"
        );
    }
}

fn prepare_words(snapshot: SourceSnapshot, summary: &mut SyncSummary) -> Vec<PreparedWord> {
    let SourceSnapshot {
        words,
        mut translations_by_word_id,
        aspects_by_word_id,
        mut forms_by_word_id,
    } = snapshot;
    let mut prepared = Vec::with_capacity(words.len());

    for source_word in words {
        if source_word.disabled {
            summary.disabled_skipped += 1;
            debug!(
                external_id = %source_word.id,
                russian = %source_word.russian,
                "Disabled OpenRussian word skipped"
            );
            continue;
        }

        prepared.push(prepare_word(
            source_word,
            &mut translations_by_word_id,
            &aspects_by_word_id,
            &mut forms_by_word_id,
        ));
    }

    prepared
}

fn prepare_word(
    source_word: SourceWord,
    translations_by_word_id: &mut HashMap<
        String,
        Vec<crate::application::ports::SourceTranslation>,
    >,
    aspects_by_word_id: &HashMap<String, Option<String>>,
    forms_by_word_id: &mut HashMap<String, Vec<SourceForm>>,
) -> PreparedWord {
    let result = (|| -> Result<ImportedWord> {
        let source_type = source_word.source_type.as_deref().map(str::trim);
        let word_type = match source_type {
            None | Some("") => None,
            Some("adjective" | "adverb" | "expression" | "noun" | "other" | "pronoun" | "verb") => {
                source_type.map(str::to_owned)
            }
            Some(unknown) => anyhow::bail!("unsupported OpenRussian word type: {unknown}"),
        };

        let aspect = if word_type.as_deref() == Some("verb") {
            let aspect = aspects_by_word_id
                .get(&source_word.id)
                .and_then(Option::as_deref);
            match aspect {
                None | Some("") => None,
                Some("imperfective" | "perfective" | "both") => aspect.map(str::to_owned),
                Some(unknown) => anyhow::bail!("unsupported OpenRussian verb aspect: {unknown}"),
            }
        } else {
            None
        };

        let mut source_translations = translations_by_word_id
            .remove(&source_word.id)
            .unwrap_or_default();
        source_translations.sort_by(|left, right| {
            compare_position_nulls_last(left.position, right.position)
                .then_with(|| left.id.cmp(&right.id))
        });
        let translations = source_translations
            .into_iter()
            .filter(|translation| !translation.text.trim().is_empty())
            .map(|translation| Translation {
                language: "en".to_owned(),
                text: translation.text,
                position: translation.position,
            })
            .collect();

        let source_forms = forms_by_word_id.remove(&source_word.id).unwrap_or_default();
        let forms = forms_from_source(word_type.as_deref(), source_forms)?;

        ImportedWord::new(
            source_word.id.clone(),
            source_word.russian.clone(),
            word_type,
            aspect,
            translations,
            source_word.usage,
            source_word.audio_url,
            forms,
        )
    })();

    match result {
        Ok(word) => PreparedWord::Ready(word),
        Err(error) => PreparedWord::Invalid {
            external_id: source_word.id,
            message: format!("{error:#}"),
        },
    }
}

fn compare_position_nulls_last(left: Option<i64>, right: Option<i64>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn forms_from_source(
    word_type: Option<&str>,
    source_forms: Vec<SourceForm>,
) -> Result<Option<WordForms>> {
    let mut forms_by_type = HashMap::<String, Vec<SourceForm>>::new();
    for form in source_forms {
        if !WORD_FORM_TYPES.contains(&form.form_type.as_str()) {
            anyhow::bail!("unsupported OpenRussian word form type: {}", form.form_type);
        }
        if form.form_bare.trim().is_empty() {
            continue;
        }
        forms_by_type
            .entry(form.form_type.clone())
            .or_default()
            .push(form);
    }

    if forms_by_type.is_empty() {
        return Ok(None);
    }
    if word_type.is_none() {
        anyhow::bail!("OpenRussian word has forms but no word type");
    }

    let mut forms = WORD_FORM_TYPES
        .iter()
        .map(|form_type| ((*form_type).to_owned(), None))
        .collect::<WordForms>();
    for (form_type, mut source_forms) in forms_by_type {
        source_forms.sort_by(|left, right| {
            compare_position_nulls_last(left.position, right.position)
                .then_with(|| left.id.cmp(&right.id))
        });
        let combined = source_forms
            .into_iter()
            .map(|form| form.form_bare)
            .collect::<Vec<_>>()
            .join("; ");
        forms.insert(form_type, Some(combined));
    }

    Ok(Some(forms))
}

#[cfg(test)]
mod tests {
    use super::{PreparedWord, SyncSummary, forms_from_source, prepare_words};
    use crate::application::ports::{SourceForm, SourceSnapshot, SourceTranslation, SourceWord};
    use crate::domain::word::ImportedWord;
    use std::collections::HashMap;

    fn prepare_imported_word(
        audio_url: Option<&str>,
        usage: Option<&str>,
        forms: Vec<SourceForm>,
        translations: Vec<SourceTranslation>,
    ) -> ImportedWord {
        let snapshot = SourceSnapshot {
            words: vec![SourceWord {
                id: "1".to_owned(),
                russian: "дом".to_owned(),
                source_type: Some("noun".to_owned()),
                disabled: false,
                audio_url: audio_url.map(str::to_owned),
                usage: usage.map(str::to_owned),
            }],
            translations_by_word_id: HashMap::from([("1".to_owned(), translations)]),
            aspects_by_word_id: HashMap::new(),
            forms_by_word_id: HashMap::from([("1".to_owned(), forms)]),
        };
        let mut summary = SyncSummary::default();
        let mut prepared = prepare_words(snapshot, &mut summary);
        match prepared.pop().expect("one active word is prepared") {
            PreparedWord::Ready(word) => word,
            PreparedWord::Invalid { message, .. } => panic!("word should be valid: {message}"),
        }
    }

    fn source_translation(id: i64, position: Option<i64>, text: &str) -> SourceTranslation {
        SourceTranslation {
            id,
            position,
            text: text.to_owned(),
        }
    }

    #[test]
    fn word_request_contains_audio_usage_forms_and_translation_position() {
        let word = prepare_imported_word(
            Some("https://example.org/audio.mp3"),
            Some("Usage in English"),
            vec![
                SourceForm {
                    id: 10,
                    position: Some(1),
                    form_type: "ru_noun_sg_nom".to_owned(),
                    form_bare: "дома".to_owned(),
                },
                SourceForm {
                    id: 2,
                    position: Some(1),
                    form_type: "ru_noun_sg_nom".to_owned(),
                    form_bare: "дом".to_owned(),
                },
            ],
            vec![source_translation(1, Some(5), "house")],
        );

        let request = serde_json::to_value(word.request()).expect("request serializes");

        assert_eq!(request["audioUrl"], "https://example.org/audio.mp3");
        assert_eq!(request["usage"], "Usage in English");
        assert_eq!(request["forms"]["ru_noun_sg_nom"], "дом; дома");
        assert!(request["forms"]["ru_noun_pl_nom"].is_null());
        assert_eq!(request["translations"][0]["position"], 5);
    }

    #[test]
    fn checksum_changes_when_audio_usage_or_forms_change() {
        let baseline = prepare_imported_word(
            None,
            None,
            vec![],
            vec![source_translation(1, Some(5), "house")],
        );
        let with_audio = prepare_imported_word(
            Some("https://example.org/audio.mp3"),
            None,
            vec![],
            vec![source_translation(1, Some(5), "house")],
        );
        let with_usage = prepare_imported_word(
            None,
            Some("Usage in English"),
            vec![],
            vec![source_translation(1, Some(5), "house")],
        );
        let position_changed = prepare_imported_word(
            None,
            None,
            vec![],
            vec![source_translation(1, Some(6), "house")],
        );
        let with_forms = prepare_imported_word(
            None,
            None,
            vec![SourceForm {
                id: 1,
                position: Some(1),
                form_type: "ru_noun_sg_nom".to_owned(),
                form_bare: "дом".to_owned(),
            }],
            vec![source_translation(1, Some(5), "house")],
        );

        assert_ne!(baseline.checksum, with_audio.checksum);
        assert_ne!(baseline.checksum, with_usage.checksum);
        assert_ne!(baseline.checksum, with_forms.checksum);
        assert_eq!(baseline.checksum, position_changed.checksum);
    }

    #[test]
    fn same_position_translations_use_numeric_source_id_order() {
        let word = prepare_imported_word(
            None,
            None,
            vec![],
            vec![
                source_translation(10, Some(1), "ten"),
                source_translation(2, Some(1), "two"),
            ],
        );
        let request = serde_json::to_value(word.request()).expect("request serializes");

        assert_eq!(request["translations"][0]["text"], "two");
        assert_eq!(request["translations"][1]["text"], "ten");
    }

    #[test]
    fn null_translation_positions_sort_after_real_maximum_position() {
        let word = prepare_imported_word(
            None,
            None,
            vec![],
            vec![
                source_translation(1, None, "null"),
                source_translation(2, Some(i64::MAX), "maximum"),
            ],
        );
        let request = serde_json::to_value(word.request()).expect("request serializes");

        assert_eq!(request["translations"][0]["text"], "maximum");
        assert_eq!(request["translations"][1]["text"], "null");
    }

    #[test]
    fn null_form_positions_sort_after_real_maximum_position() {
        let forms = forms_from_source(
            Some("noun"),
            vec![
                SourceForm {
                    id: 1,
                    position: None,
                    form_type: "ru_noun_sg_nom".to_owned(),
                    form_bare: "null".to_owned(),
                },
                SourceForm {
                    id: 2,
                    position: Some(i64::MAX),
                    form_type: "ru_noun_sg_nom".to_owned(),
                    form_bare: "maximum".to_owned(),
                },
            ],
        )
        .expect("forms are valid")
        .expect("forms are present");

        assert_eq!(forms["ru_noun_sg_nom"].as_deref(), Some("maximum; null"));
    }

    #[test]
    fn form_type_is_validated_even_when_form_text_is_blank() {
        let error = forms_from_source(
            Some("noun"),
            vec![SourceForm {
                id: 1,
                position: Some(1),
                form_type: "unsupported_form".to_owned(),
                form_bare: " ".to_owned(),
            }],
        )
        .expect_err("unsupported form type is rejected");

        assert!(
            error
                .to_string()
                .contains("unsupported OpenRussian word form type")
        );
    }

    #[test]
    fn nonempty_forms_without_word_type_are_rejected() {
        let error = forms_from_source(
            None,
            vec![SourceForm {
                id: 1,
                position: Some(1),
                form_type: "ru_noun_sg_nom".to_owned(),
                form_bare: "дом".to_owned(),
            }],
        )
        .expect_err("forms require word type");

        assert!(error.to_string().contains("forms but no word type"));
    }
}
