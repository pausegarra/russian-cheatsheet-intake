use crate::domain::word::{ImportedWord, Translation};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;

#[derive(Serialize)]
struct CanonicalWord<'a> {
    aspect: Option<&'a str>,
    #[serde(rename = "audioUrl")]
    audio_url: Option<&'a str>,
    forms: Option<&'a crate::domain::word::WordForms>,
    russian: &'a str,
    translations: Vec<CanonicalTranslation<'a>>,
    #[serde(rename = "type")]
    word_type: Option<&'a str>,
    usage: Option<&'a str>,
}

#[derive(Serialize)]
struct CanonicalTranslation<'a> {
    language: &'a str,
    text: &'a str,
}

pub fn calculate(word: &ImportedWord) -> anyhow::Result<String> {
    let mut translations: Vec<&Translation> = word.translations.iter().collect();
    translations.sort_by(|left, right| {
        compare_java_strings(&left.language, &right.language)
            .then_with(|| compare_java_strings(&left.text, &right.text))
    });

    let canonical = CanonicalWord {
        aspect: word.aspect.as_deref(),
        audio_url: word.audio_url.as_deref(),
        forms: word.forms.as_ref(),
        russian: &word.russian,
        translations: translations
            .into_iter()
            .map(|translation| CanonicalTranslation {
                language: &translation.language,
                text: &translation.text,
            })
            .collect(),
        word_type: word.word_type.as_deref(),
        usage: word.usage.as_deref(),
    };

    let mut json = serde_json::to_vec(&canonical)?;
    uppercase_unicode_escape_hex(&mut json);
    let digest = Sha256::digest(json);
    Ok(format!("{digest:x}"))
}

fn uppercase_unicode_escape_hex(json: &mut [u8]) {
    // Jackson's canonical checksum JSON uses uppercase hex digits in `\u` escapes.
    let mut index = 0;
    while index < json.len() {
        if json[index] != b'\\' {
            index += 1;
            continue;
        }

        if json.get(index + 1) == Some(&b'u') && json.len() - index >= 6 {
            for digit in &mut json[index + 2..index + 6] {
                *digit = digit.to_ascii_uppercase();
            }
            index += 6;
        } else {
            index = (index + 2).min(json.len());
        }
    }
}

fn compare_java_strings(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_control_characters_with_jackson_compatible_escaping() {
        let word = ImportedWord::new(
            "1".to_owned(),
            "тест".to_owned(),
            Some("other".to_owned()),
            None,
            vec![Translation {
                language: "en".to_owned(),
                text: "test".to_owned(),
                position: Some(0),
            }],
            Some("line\u{000f}break".to_owned()),
            None,
            None,
        )
        .expect("word checksum calculates");

        assert_eq!(
            word.checksum,
            "788ca2a6fd87438c570fc9b8fc2cab9f45ced61e24f3a7b6b2fa962678c5519e"
        );
    }
}
