use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use reqwest::blocking::{Client, Response};
use reqwest::header::{CONTENT_LENGTH, ETAG, LAST_MODIFIED};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, copy};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

const KAIKKI_RUSSIAN_WIKTIONARY_URL: &str =
    "https://kaikki.org/dictionary/downloads/ru/ru-extract.jsonl.gz";
const CACHE_FILENAME: &str = "ru-extract.jsonl.gz";
const CACHE_METADATA_FILENAME: &str = "ru-extract.metadata.json";

pub(super) fn find_translations(
    client: &Client,
    cache_dir: &Path,
    unresolved_words: &HashSet<String>,
) -> Result<HashMap<String, Vec<String>>> {
    if unresolved_words.is_empty() {
        return Ok(HashMap::new());
    }

    let mut dump = cached_dump(client, cache_dir)?;
    let results = match parse_dump(&dump.path, unresolved_words) {
        Ok(results) => results,
        Err(error) => {
            dump.invalidate()?;
            return Err(error);
        }
    };
    dump.commit()?;

    info!(
        queried_words = unresolved_words.len(),
        translated_words = results.len(),
        "Kaikki fallback lookup complete"
    );
    Ok(results)
}

fn parse_dump(
    dump_path: &Path,
    unresolved_words: &HashSet<String>,
) -> Result<HashMap<String, Vec<String>>> {
    let dump = File::open(dump_path)
        .with_context(|| format!("could not open Kaikki cache {}", dump_path.display()))?;
    let mut reader = BufReader::new(GzDecoder::new(dump));
    let mut results = HashMap::<String, Vec<String>>::new();
    let mut line = Vec::new();
    let mut line_number = 0;

    loop {
        line.clear();
        if reader
            .read_until(b'\n', &mut line)
            .context("failed while reading Kaikki Russian Wiktionary dump")?
            == 0
        {
            break;
        }
        line_number += 1;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }

        let entry: KaikkiEntry = serde_json::from_slice(&line)
            .with_context(|| format!("invalid Kaikki JSONL entry at line {line_number}"))?;
        if entry.word.is_empty() {
            bail!("Kaikki JSONL entry at line {line_number} has no word or title");
        }
        let key = super::lookup_key(&entry.word);
        if !unresolved_words.contains(&key) {
            continue;
        }

        let entry_translations = entry
            .translations
            .iter()
            .chain(
                entry
                    .senses
                    .iter()
                    .flat_map(|sense| sense.translations.iter()),
            )
            .filter(|translation| translation.is_spanish())
            .flat_map(|translation| translation.spanish_words())
            .collect::<Vec<_>>();
        if entry_translations.is_empty() {
            continue;
        }

        let translations = results.entry(key).or_default();
        for translation in entry_translations {
            super::push_unique(translations, &translation);
        }
    }

    Ok(results)
}

fn cached_dump(client: &Client, cache_dir: &Path) -> Result<CachedDump> {
    fs::create_dir_all(cache_dir)
        .with_context(|| format!("could not create Kaikki cache {}", cache_dir.display()))?;
    let cache_path = cache_dir.join(CACHE_FILENAME);
    let metadata_path = cache_dir.join(CACHE_METADATA_FILENAME);
    let cached_metadata = read_metadata(&metadata_path);

    let remote_metadata = match client.head(KAIKKI_RUSSIAN_WIKTIONARY_URL).send() {
        Ok(response) if response.status().is_success() => {
            CacheMetadata::from_headers(response.headers())
        }
        Ok(response) if response.status().as_u16() == 405 => {
            if cache_path.is_file() {
                warn!("Kaikki does not support HEAD; reusing existing local dump cache");
                return Ok(CachedDump::existing(cache_path, metadata_path));
            }
            warn!("Kaikki does not support HEAD; downloading without version check");
            CacheMetadata::default()
        }
        Ok(response) => {
            bail!(
                "Kaikki version check failed with HTTP {}",
                response.status()
            );
        }
        Err(error) if cache_path.is_file() => {
            warn!(error = %error, "Kaikki version check failed; reusing existing local dump cache");
            return Ok(CachedDump::existing(cache_path, metadata_path));
        }
        Err(error) => {
            return Err(error).context("failed to check Kaikki Russian Wiktionary dump version");
        }
    };
    if cache_path.is_file()
        && cached_metadata
            .as_ref()
            .is_some_and(|cached| cached.matches_remote(&remote_metadata))
    {
        info!(path = %cache_path.display(), "Reusing current Kaikki dump cache");
        return Ok(CachedDump::existing(cache_path, metadata_path));
    }

    info!("Downloading Kaikki Russian Wiktionary fallback data");
    let mut response = client
        .get(KAIKKI_RUSSIAN_WIKTIONARY_URL)
        .send()
        .context("failed to download Kaikki Russian Wiktionary dump")?
        .error_for_status()
        .context("Kaikki rejected Russian Wiktionary dump download")?;
    let downloaded_metadata = CacheMetadata::from_headers(response.headers());
    let metadata = downloaded_metadata.merge_missing(&remote_metadata);
    let temporary_path = temporary_path(cache_dir)?;
    let download_result = save_download(&mut response, &temporary_path);
    if let Err(error) = download_result {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }

    Ok(CachedDump {
        path: temporary_path,
        cache_path,
        metadata_path,
        metadata,
        pending_install: true,
    })
}

struct CachedDump {
    path: PathBuf,
    cache_path: PathBuf,
    metadata_path: PathBuf,
    metadata: CacheMetadata,
    pending_install: bool,
}

impl CachedDump {
    fn existing(cache_path: PathBuf, metadata_path: PathBuf) -> Self {
        Self {
            path: cache_path.clone(),
            cache_path,
            metadata_path,
            metadata: CacheMetadata::default(),
            pending_install: false,
        }
    }

    fn commit(&mut self) -> Result<()> {
        if !self.pending_install {
            return Ok(());
        }
        if self.cache_path.exists() {
            fs::remove_file(&self.cache_path).with_context(|| {
                format!(
                    "could not replace Kaikki cache {}",
                    self.cache_path.display()
                )
            })?;
        }
        fs::rename(&self.path, &self.cache_path).with_context(|| {
            format!(
                "could not install Kaikki cache {}",
                self.cache_path.display()
            )
        })?;
        self.path = self.cache_path.clone();
        self.pending_install = false;
        write_metadata(&self.metadata_path, &self.metadata)?;
        info!(path = %self.cache_path.display(), "Kaikki dump cache updated");
        Ok(())
    }

    fn invalidate(&self) -> Result<()> {
        if self.pending_install {
            return Ok(());
        }
        for path in [&self.cache_path, &self.metadata_path] {
            if path.exists() {
                fs::remove_file(path).with_context(|| {
                    format!("could not remove invalid Kaikki cache {}", path.display())
                })?;
            }
        }
        Ok(())
    }
}

impl Drop for CachedDump {
    fn drop(&mut self) {
        if self.pending_install {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn save_download(response: &mut Response, path: &Path) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("could not create temporary Kaikki file {}", path.display()))?;
    copy(response, &mut file).context("failed to save Kaikki Russian Wiktionary dump")?;
    file.sync_all()
        .context("failed to flush Kaikki Russian Wiktionary dump")?;
    Ok(())
}

fn temporary_path(cache_dir: &Path) -> Result<PathBuf> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock predates Unix epoch")?
        .as_nanos();
    Ok(cache_dir.join(format!(
        "{CACHE_FILENAME}.{}.{}.tmp",
        std::process::id(),
        nonce
    )))
}

fn read_metadata(path: &Path) -> Option<CacheMetadata> {
    let data = fs::read(path).ok()?;
    serde_json::from_slice(&data).ok()
}

fn write_metadata(path: &Path, metadata: &CacheMetadata) -> Result<()> {
    let bytes =
        serde_json::to_vec(metadata).context("could not serialize Kaikki cache metadata")?;
    fs::write(path, bytes)
        .with_context(|| format!("could not write Kaikki metadata {}", path.display()))
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct CacheMetadata {
    etag: Option<String>,
    last_modified: Option<String>,
    content_length: Option<u64>,
}

impl CacheMetadata {
    fn from_headers(headers: &reqwest::header::HeaderMap) -> Self {
        Self {
            etag: header_value(headers, ETAG),
            last_modified: header_value(headers, LAST_MODIFIED),
            content_length: header_value(headers, CONTENT_LENGTH)
                .and_then(|value| value.parse().ok()),
        }
    }

    fn matches_remote(&self, remote: &Self) -> bool {
        if let Some(remote_etag) = &remote.etag {
            return self.etag.as_ref() == Some(remote_etag);
        }
        if let Some(remote_last_modified) = &remote.last_modified {
            return self.last_modified.as_ref() == Some(remote_last_modified);
        }
        false
    }

    fn merge_missing(&self, fallback: &Self) -> Self {
        Self {
            etag: self.etag.clone().or_else(|| fallback.etag.clone()),
            last_modified: self
                .last_modified
                .clone()
                .or_else(|| fallback.last_modified.clone()),
            content_length: self.content_length.or(fallback.content_length),
        }
    }
}

fn header_value(
    headers: &reqwest::header::HeaderMap,
    name: reqwest::header::HeaderName,
) -> Option<String> {
    headers.get(name)?.to_str().ok().map(str::to_owned)
}

#[derive(Deserialize)]
struct KaikkiEntry {
    #[serde(default, alias = "title")]
    word: String,
    #[serde(default)]
    translations: Vec<KaikkiTranslation>,
    #[serde(default)]
    senses: Vec<KaikkiSense>,
}

#[derive(Deserialize)]
struct KaikkiSense {
    #[serde(default)]
    translations: Vec<KaikkiTranslation>,
}

#[derive(Deserialize)]
struct KaikkiTranslation {
    #[serde(default)]
    lang_code: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    lang: Option<String>,
    #[serde(default)]
    word: Option<String>,
    #[serde(default)]
    alt: Option<String>,
}

impl KaikkiTranslation {
    fn is_spanish(&self) -> bool {
        [
            self.lang_code.as_deref(),
            self.code.as_deref(),
            self.lang.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|language| {
            matches!(
                language.to_lowercase().as_str(),
                "es" | "spa" | "spanish" | "español" | "испанский"
            )
        })
    }

    fn spanish_words(&self) -> impl Iterator<Item = String> + '_ {
        self.word
            .iter()
            .chain(self.alt.iter())
            .map(|word| word.trim())
            .filter(|word| !word.is_empty())
            .map(str::to_owned)
    }
}
