use crate::application::ports::{ApiWord, ApiWriteError, WordsApi, WriteEffect, WriteResult};
use crate::domain::word::ImportedWord;
use crate::infrastructure::keycloak::JwtKeycloakContextHolder;
use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};
use url::Url;

const PAGE_SIZE: usize = 1000;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

pub struct CheatsheetApi {
    http: Client,
    base_url: Url,
    keycloak: Arc<JwtKeycloakContextHolder>,
}

impl CheatsheetApi {
    pub fn new(base_url: &str, keycloak: Arc<JwtKeycloakContextHolder>) -> Result<Self> {
        let mut base_url = Url::parse(base_url).context("invalid Russian Cheatsheet API URL")?;
        if !matches!(base_url.scheme(), "http" | "https") {
            bail!("Russian Cheatsheet API URL must use HTTP or HTTPS");
        }
        if base_url.query().is_some() || base_url.fragment().is_some() {
            bail!("Russian Cheatsheet API URL must not include a query or fragment");
        }
        let path = base_url.path().trim_end_matches('/');
        base_url.set_path(&format!("{path}/"));

        let http = Client::builder()
            .user_agent(concat!(
                "russian-cheatsheet-intake/",
                env!("CARGO_PKG_VERSION")
            ))
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("could not create Russian Cheatsheet API HTTP client")?;

        Ok(Self {
            http,
            base_url,
            keycloak,
        })
    }

    fn find_all(&self) -> Result<HashMap<String, ApiWord>> {
        let mut words = HashMap::new();
        let mut page = 0usize;
        info!("Starting Russian Cheatsheet API word read");

        loop {
            debug!(page, page_size = PAGE_SIZE, "Requesting API word page");
            let mut url = self.base_url.join("words")?;
            url.query_pairs_mut()
                .append_pair("page", &page.to_string())
                .append_pair("perPage", &PAGE_SIZE.to_string());
            let token = self.keycloak.get_token()?;

            let response: WordPage = self
                .http
                .get(url)
                .bearer_auth(token)
                .send()
                .with_context(|| {
                    format!("failed to read Russian Cheatsheet API words page {page}")
                })?
                .error_for_status()
                .with_context(|| format!("Russian Cheatsheet API rejected words page {page}"))?
                .json()
                .with_context(|| {
                    format!("invalid Russian Cheatsheet API response on page {page}")
                })?;

            let page_word_count = response.data.len();
            let has_next_page = response.has_next_page;
            for item in response.data {
                let Some(external_id) = item.external_id else {
                    continue;
                };
                let api_word = ApiWord {
                    id: item.id,
                    external_id: external_id.clone(),
                    checksum: item.checksum,
                };
                if words.insert(external_id.clone(), api_word).is_some() {
                    bail!("Russian Cheatsheet API returned duplicate externalId={external_id}");
                }
            }

            info!(
                page,
                words = page_word_count,
                has_next_page,
                "Russian Cheatsheet API page received"
            );
            if !has_next_page {
                break;
            }
            page += 1;
        }

        info!(
            words = words.len(),
            "Russian Cheatsheet API word read complete"
        );
        Ok(words)
    }

    fn send_word(
        &self,
        method: Method,
        url: Url,
        word: &ImportedWord,
    ) -> std::result::Result<ApiWord, ApiWriteError> {
        let request = word.request();
        let token = self.keycloak.get_token().map_err(api_error)?;
        let response = self
            .http
            .request(method, url)
            .bearer_auth(token)
            .json(&request)
            .send()
            .map_err(|error| ApiWriteError {
                message: format!("request failed: {error}"),
                outcome_unknown: error.is_timeout() || error.is_connect(),
                fatal: false,
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .unwrap_or_else(|error| format!("<could not read body: {error}>"));
            return Err(ApiWriteError {
                message: format!("API returned {status}: {body}"),
                outcome_unknown: status == StatusCode::CONFLICT || status.is_server_error(),
                fatal: status == StatusCode::UNAUTHORIZED
                    || status == StatusCode::FORBIDDEN
                    || status.is_server_error(),
            });
        }

        let item: WordResponse = response.json().map_err(|error| ApiWriteError {
            message: format!("API returned an unreadable word response: {error}"),
            outcome_unknown: true,
            fatal: false,
        })?;
        Ok(ApiWord {
            id: item.id,
            external_id: item.external_id.unwrap_or_default(),
            checksum: item.checksum,
        })
    }

    fn checked_result(
        &self,
        api_word: ApiWord,
        word: &ImportedWord,
        expected_id: Option<&str>,
    ) -> std::result::Result<ApiWord, ApiWriteError> {
        if api_word.external_id != word.external_id {
            return Err(ApiWriteError {
                message: format!(
                    "API returned externalId={} for requested externalId={}",
                    api_word.external_id, word.external_id
                ),
                outcome_unknown: false,
                fatal: true,
            });
        }
        if expected_id.is_some_and(|expected| expected != api_word.id) {
            return Err(ApiWriteError {
                message: format!("API changed id for externalId={}", word.external_id),
                outcome_unknown: false,
                fatal: true,
            });
        }
        if api_word.checksum.as_deref() != Some(word.checksum.as_str()) {
            return Err(ApiWriteError {
                message: format!(
                    "API checksum mismatch for externalId={}: expected {}, received {}",
                    word.external_id,
                    word.checksum,
                    api_word.checksum.as_deref().unwrap_or("<null>")
                ),
                outcome_unknown: false,
                fatal: true,
            });
        }
        Ok(api_word)
    }

    fn reconcile(
        &self,
        word: &ImportedWord,
    ) -> std::result::Result<Option<ApiWord>, ApiWriteError> {
        self.find_all()
            .map(|mut words| words.remove(&word.external_id))
            .map_err(|error| ApiWriteError {
                message: format!(
                    "could not reconcile externalId={}: {error:#}",
                    word.external_id
                ),
                outcome_unknown: true,
                fatal: true,
            })
    }

    fn word_url(&self, id: Option<&str>) -> Result<Url> {
        let mut url = self.base_url.join("words")?;
        if let Some(id) = id {
            url.path_segments_mut()
                .map_err(|_| anyhow::anyhow!("API URL cannot accept word ID"))?
                .push(id);
        }
        Ok(url)
    }
}

impl WordsApi for CheatsheetApi {
    fn find_all_by_external_id(&self) -> Result<HashMap<String, ApiWord>> {
        self.find_all()
    }

    fn create(&self, word: &ImportedWord) -> std::result::Result<WriteResult, ApiWriteError> {
        match self.send_word(Method::POST, self.word_url(None).map_err(api_error)?, word) {
            Ok(api_word) => {
                let api_word = self.checked_result(api_word, word, None)?;
                Ok(WriteResult {
                    word: api_word,
                    effect: WriteEffect::Created,
                })
            }
            Err(error) if error.outcome_unknown => {
                warn!(
                    external_id = %word.external_id,
                    "Create outcome is unknown; reconciling against API"
                );
                let Some(existing) = self.reconcile(word)? else {
                    return Err(ApiWriteError {
                        message: format!(
                            "{}; externalId={} not found after reconciliation",
                            error.message, word.external_id
                        ),
                        outcome_unknown: true,
                        fatal: true,
                    });
                };
                if existing.checksum.as_deref() == Some(word.checksum.as_str()) {
                    return Ok(WriteResult {
                        word: existing,
                        effect: WriteEffect::Created,
                    });
                }
                self.update(&existing, word)
            }
            Err(error) => Err(error),
        }
    }

    fn update(
        &self,
        existing: &ApiWord,
        word: &ImportedWord,
    ) -> std::result::Result<WriteResult, ApiWriteError> {
        let url = self.word_url(Some(&existing.id)).map_err(api_error)?;
        match self.send_word(Method::PUT, url, word) {
            Ok(api_word) => {
                let api_word = self.checked_result(api_word, word, Some(&existing.id))?;
                Ok(WriteResult {
                    word: api_word,
                    effect: WriteEffect::Updated,
                })
            }
            Err(error) if error.outcome_unknown => {
                warn!(
                    external_id = %word.external_id,
                    "Update outcome is unknown; reconciling against API"
                );
                let Some(current) = self.reconcile(word)? else {
                    return Err(ApiWriteError {
                        message: format!(
                            "{}; externalId={} not found after reconciliation",
                            error.message, word.external_id
                        ),
                        outcome_unknown: true,
                        fatal: true,
                    });
                };
                let current = self.checked_result(current, word, Some(&existing.id))?;
                Ok(WriteResult {
                    word: current,
                    effect: WriteEffect::Updated,
                })
            }
            Err(error) => Err(error),
        }
    }
}

fn api_error(error: anyhow::Error) -> ApiWriteError {
    ApiWriteError {
        message: format!("{error:#}"),
        outcome_unknown: false,
        fatal: true,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WordPage {
    data: Vec<WordReference>,
    has_next_page: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WordReference {
    id: String,
    external_id: Option<String>,
    checksum: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WordResponse {
    id: String,
    external_id: Option<String>,
    checksum: Option<String>,
}
