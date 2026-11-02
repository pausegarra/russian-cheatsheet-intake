mod freedict;
mod kaikki;

use crate::application::ports::SpanishTranslationSource;
use anyhow::{Context, Result};
use reqwest::blocking::Client;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;
use tracing::info;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20 * 60);

pub struct SpanishTranslationProvider {
    http: Client,
    cache_dir: PathBuf,
}

impl SpanishTranslationProvider {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .user_agent(concat!(
                "russian-cheatsheet-intake/",
                env!("CARGO_PKG_VERSION")
            ))
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("could not create dictionary HTTP client")?;
        let cache_dir = std::env::current_dir()
            .context("could not resolve current directory for dictionary cache")?
            .join(".cache")
            .join("russian-cheatsheet-intake");

        Ok(Self { http, cache_dir })
    }
}

impl SpanishTranslationSource for SpanishTranslationProvider {
    fn find_spanish_translations(
        &self,
        russian_words: &[String],
    ) -> Result<HashMap<String, Vec<String>>> {
        let mut originals_by_key = HashMap::<String, Vec<String>>::new();
        for russian_word in russian_words {
            let key = lookup_key(russian_word);
            if !key.is_empty() {
                originals_by_key
                    .entry(key)
                    .or_default()
                    .push(russian_word.clone());
            }
        }
        let unique_russian_words = russian_words.iter().collect::<HashSet<_>>().len();
        if originals_by_key.is_empty() {
            return Ok(HashMap::new());
        }

        info!(
            headwords = originals_by_key.len(),
            "Loading FreeDict RU-ES data"
        );
        let freedict = freedict::load(&self.http)?;
        let unresolved = originals_by_key
            .keys()
            .filter(|key| freedict.get(*key).is_none_or(Vec::is_empty))
            .cloned()
            .collect::<HashSet<_>>();
        let kaikki = if unresolved.is_empty() {
            HashMap::new()
        } else {
            info!(
                headwords = unresolved.len(),
                "FreeDict misses; loading Kaikki fallback data"
            );
            kaikki::find_translations(&self.http, &self.cache_dir, &unresolved)?
        };

        let mut results = HashMap::new();
        for (key, original_words) in originals_by_key {
            let translations = freedict
                .get(&key)
                .filter(|translations| !translations.is_empty())
                .or_else(|| kaikki.get(&key));
            if let Some(translations) = translations {
                for original_word in original_words {
                    results.insert(original_word, translations.clone());
                }
            }
        }

        info!(
            headwords = unique_russian_words,
            translated_headwords = results.len(),
            fallback_headwords = unresolved.len(),
            pending_headwords = unique_russian_words.saturating_sub(results.len()),
            "Spanish translation lookup complete"
        );
        Ok(results)
    }
}

fn lookup_key(word: &str) -> String {
    word.chars()
        .filter(|character| !matches!(character, '\u{0301}' | '\u{00b4}'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn push_unique(translations: &mut Vec<String>, translation: &str) {
    if !translations.iter().any(|existing| existing == translation) {
        translations.push(translation.to_owned());
    }
}
