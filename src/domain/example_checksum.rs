use crate::domain::checksum::{compare_java_strings, uppercase_unicode_escape_hex};
use crate::domain::example::{ExampleTranslation, ImportedExample};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CanonicalExample<'a> {
    audio_url: Option<&'a str>,
    contributor: Option<&'a str>,
    linked_word_ids: &'a [String],
    russian: &'a str,
    translations: Vec<CanonicalTranslation<'a>>,
}

#[derive(Serialize)]
struct CanonicalTranslation<'a> {
    language: &'a str,
    position: i32,
    text: &'a str,
}

pub fn calculate(example: &ImportedExample) -> anyhow::Result<String> {
    let mut translations: Vec<&ExampleTranslation> = example.translations.iter().collect();
    translations.sort_by(|left, right| {
        compare_java_strings(&left.language, &right.language)
            .then_with(|| left.position.cmp(&right.position))
            .then_with(|| compare_java_strings(&left.text, &right.text))
    });

    let canonical = CanonicalExample {
        audio_url: example.audio_url.as_deref(),
        contributor: example.contributor.as_deref(),
        linked_word_ids: &example.linked_word_ids,
        russian: &example.russian,
        translations: translations
            .into_iter()
            .map(|translation| CanonicalTranslation {
                language: &translation.language,
                position: translation.position,
                text: &translation.text,
            })
            .collect(),
    };

    let mut json = serde_json::to_vec(&canonical)?;
    uppercase_unicode_escape_hex(&mut json);
    let digest = Sha256::digest(json);
    Ok(format!("{digest:x}"))
}

pub(crate) fn sort_linked_word_ids(linked_word_ids: &mut Vec<String>) {
    linked_word_ids.sort_by(|left, right| compare_java_uuids(left, right));
    linked_word_ids.dedup();
}

fn compare_java_uuids(left: &str, right: &str) -> Ordering {
    match (uuid_as_java_order(left), uuid_as_java_order(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => compare_java_strings(left, right),
    }
}

fn uuid_as_java_order(value: &str) -> Option<(i64, i64)> {
    let compact = value.replace('-', "");
    if compact.len() != 32 {
        return None;
    }
    let bits = u128::from_str_radix(&compact, 16).ok()?;
    Some(((bits >> 64) as u64 as i64, bits as u64 as i64))
}
