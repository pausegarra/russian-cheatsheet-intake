use crate::application::ports::{
    ApiExample, ApiWord, ApiWriteError, ExampleWriteResult, ExamplesApi, WordsApi, WriteEffect,
    WriteResult,
};
use crate::domain::example::ImportedExample;
use crate::domain::word::ImportedWord;
use crate::infrastructure::keycloak::JwtKeycloakContextHolder;
use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};
use url::Url;

const PAGE_SIZE: usize = 1000;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_RATE_LIMIT_RETRIES: u32 = 4;
const MAX_RATE_LIMIT_DELAY: Duration = Duration::from_secs(30);

#[derive(Clone)]
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

    fn find_all_examples(&self) -> Result<HashMap<String, ApiExample>> {
        let mut examples = HashMap::new();
        let mut page = 0usize;
        info!("Starting Russian Cheatsheet API example read");

        loop {
            debug!(page, page_size = PAGE_SIZE, "Requesting API example page");
            let mut url = self.base_url.join("examples")?;
            url.query_pairs_mut()
                .append_pair("page", &page.to_string())
                .append_pair("perPage", &PAGE_SIZE.to_string())
                .append_pair("externalIdOnly", "true");
            let token = self.keycloak.get_token()?;

            let response: ExamplePage = self
                .http
                .get(url)
                .bearer_auth(token)
                .send()
                .with_context(|| {
                    format!("failed to read Russian Cheatsheet API examples page {page}")
                })?
                .error_for_status()
                .with_context(|| format!("Russian Cheatsheet API rejected examples page {page}"))?
                .json()
                .with_context(|| {
                    format!("invalid Russian Cheatsheet API response on examples page {page}")
                })?;

            let page_example_count = response.data.len();
            let has_next_page = response.has_next_page;
            for item in response.data {
                let Some(external_id) = item.external_id else {
                    continue;
                };
                let example = ApiExample {
                    id: item.id,
                    external_id: external_id.clone(),
                    checksum: item.checksum,
                };
                if examples.insert(external_id.clone(), example).is_some() {
                    bail!(
                        "Russian Cheatsheet API returned duplicate example externalId={external_id}"
                    );
                }
            }

            info!(
                page,
                examples = page_example_count,
                has_next_page,
                "Russian Cheatsheet API example page received"
            );
            if !has_next_page {
                break;
            }
            page += 1;
        }

        info!(
            examples = examples.len(),
            "Russian Cheatsheet API example read complete"
        );
        Ok(examples)
    }

    fn send_batch_request<T: Serialize>(
        &self,
        method: Method,
        url: Url,
        request: &T,
        resource: &'static str,
        retry_rate_limits: bool,
    ) -> std::result::Result<reqwest::blocking::Response, ApiWriteError> {
        let mut rate_limit_retries = 0;
        let response = loop {
            let token = self.keycloak.get_token().map_err(api_error)?;
            let response = self
                .http
                .request(method.clone(), url.clone())
                .bearer_auth(token)
                .json(request)
                .send()
                .map_err(|error| ApiWriteError {
                    message: format!("request failed: {error}"),
                    outcome_unknown: error.is_timeout() || error.is_connect(),
                    fatal: false,
                })?;

            if !retry_rate_limits
                || response.status() != StatusCode::TOO_MANY_REQUESTS
                || rate_limit_retries >= MAX_RATE_LIMIT_RETRIES
            {
                break response;
            }

            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|header| header.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok());
            let fallback_delay =
                Duration::from_secs(1u64 << rate_limit_retries).min(MAX_RATE_LIMIT_DELAY);
            let delay = retry_after
                .map(Duration::from_secs)
                .unwrap_or(fallback_delay);
            warn!(
                resource,
                retry = rate_limit_retries + 1,
                delay_seconds = delay.as_secs(),
                "Batch write was rate limited; retrying"
            );
            std::thread::sleep(delay);
            rate_limit_retries += 1;
        };

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .unwrap_or_else(|error| format!("<could not read body: {error}>"));
            return Err(ApiWriteError {
                message: format!("API returned {status}: {body}"),
                outcome_unknown: status == StatusCode::CONFLICT
                    || status == StatusCode::REQUEST_TIMEOUT
                    || status.is_server_error(),
                fatal: status == StatusCode::UNAUTHORIZED
                    || status == StatusCode::FORBIDDEN
                    || status.is_server_error(),
            });
        }

        Ok(response)
    }

    fn validate_batch_response_count(
        &self,
        resource: &str,
        requested: usize,
        received: usize,
    ) -> std::result::Result<(), ApiWriteError> {
        if requested == received {
            return Ok(());
        }

        Err(ApiWriteError {
            message: format!("API returned {received} {resource} for batch of {requested}"),
            outcome_unknown: true,
            fatal: true,
        })
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

    fn send_example(
        &self,
        method: Method,
        url: Url,
        example: &ImportedExample,
        include_external_id: bool,
    ) -> std::result::Result<ApiExample, ApiWriteError> {
        let request = example.request(include_external_id);
        let mut rate_limit_retries = 0;
        let response = loop {
            let token = self.keycloak.get_token().map_err(api_error)?;
            let response = self
                .http
                .request(method.clone(), url.clone())
                .bearer_auth(token)
                .json(&request)
                .send()
                .map_err(|error| ApiWriteError {
                    message: format!("request failed: {error}"),
                    outcome_unknown: error.is_timeout() || error.is_connect(),
                    fatal: false,
                })?;

            if response.status() != StatusCode::TOO_MANY_REQUESTS
                || rate_limit_retries >= MAX_RATE_LIMIT_RETRIES
            {
                break response;
            }

            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|header| header.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok());
            let fallback_delay =
                Duration::from_secs(1u64 << rate_limit_retries).min(MAX_RATE_LIMIT_DELAY);
            let delay = retry_after
                .map(Duration::from_secs)
                .unwrap_or(fallback_delay);
            warn!(
                external_id = %example.external_id,
                retry = rate_limit_retries + 1,
                delay_seconds = delay.as_secs(),
                "Example write was rate limited; retrying"
            );
            std::thread::sleep(delay);
            rate_limit_retries += 1;
        };

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

        let item: ExampleResponse = response.json().map_err(|error| ApiWriteError {
            message: format!("API returned an unreadable example response: {error}"),
            outcome_unknown: true,
            fatal: false,
        })?;
        let Some(external_id) = item.external_id else {
            return Err(ApiWriteError {
                message: "API returned an example without externalId".to_owned(),
                outcome_unknown: true,
                fatal: true,
            });
        };
        Ok(ApiExample {
            id: item.id,
            external_id,
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

    fn checked_example_result(
        &self,
        api_example: ApiExample,
        example: &ImportedExample,
        expected_id: Option<&str>,
    ) -> std::result::Result<ApiExample, ApiWriteError> {
        if api_example.external_id != example.external_id {
            return Err(ApiWriteError {
                message: format!(
                    "API returned externalId={} for requested externalId={}",
                    api_example.external_id, example.external_id
                ),
                outcome_unknown: false,
                fatal: true,
            });
        }
        if expected_id.is_some_and(|expected| expected != api_example.id) {
            return Err(ApiWriteError {
                message: format!("API changed id for externalId={}", example.external_id),
                outcome_unknown: false,
                fatal: true,
            });
        }
        if api_example.checksum.as_deref() != Some(example.checksum.as_str()) {
            return Err(ApiWriteError {
                message: format!(
                    "API checksum mismatch for example externalId={}: expected {}, received {}",
                    example.external_id,
                    example.checksum,
                    api_example.checksum.as_deref().unwrap_or("<null>")
                ),
                outcome_unknown: false,
                fatal: true,
            });
        }
        Ok(api_example)
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

    fn example_url(&self, id: Option<&str>) -> Result<Url> {
        let mut url = self.base_url.join("examples")?;
        if let Some(id) = id {
            url.path_segments_mut()
                .map_err(|_| anyhow::anyhow!("API URL cannot accept example ID"))?
                .push(id);
        }
        Ok(url)
    }
}

impl WordsApi for CheatsheetApi {
    fn find_all_by_external_id(&self) -> Result<HashMap<String, ApiWord>> {
        self.find_all()
    }

    fn create_batch(
        &self,
        words: &[ImportedWord],
    ) -> std::result::Result<Vec<ApiWord>, ApiWriteError> {
        if words.is_empty() {
            return Ok(Vec::new());
        }

        let requests: Vec<_> = words.iter().map(ImportedWord::request).collect();
        let response = self.send_batch_request(
            Method::POST,
            self.base_url
                .join("words/batch")
                .map_err(|error| api_error(error.into()))?,
            &requests,
            "words",
            false,
        )?;
        let items: Vec<WordResponse> = response.json().map_err(|error| ApiWriteError {
            message: format!("API returned an unreadable words batch response: {error}"),
            outcome_unknown: true,
            fatal: false,
        })?;
        self.validate_batch_response_count("words", words.len(), items.len())?;

        words
            .iter()
            .zip(items)
            .map(|(word, item)| {
                self.checked_result(
                    ApiWord {
                        id: item.id,
                        external_id: item.external_id.unwrap_or_default(),
                        checksum: item.checksum,
                    },
                    word,
                    None,
                )
                .map_err(mark_batch_integrity_error)
            })
            .collect()
    }

    fn update_batch(
        &self,
        updates: &[(ApiWord, ImportedWord)],
    ) -> std::result::Result<Vec<ApiWord>, ApiWriteError> {
        if updates.is_empty() {
            return Ok(Vec::new());
        }

        let requests: Vec<_> = updates
            .iter()
            .map(|(existing, word)| word.update_request(&existing.id))
            .collect();
        let response = self.send_batch_request(
            Method::PUT,
            self.base_url
                .join("words/batch")
                .map_err(|error| api_error(error.into()))?,
            &requests,
            "words",
            false,
        )?;
        let items: Vec<WordResponse> = response.json().map_err(|error| ApiWriteError {
            message: format!("API returned an unreadable words batch response: {error}"),
            outcome_unknown: true,
            fatal: false,
        })?;
        self.validate_batch_response_count("words", updates.len(), items.len())?;

        updates
            .iter()
            .zip(items)
            .map(|((existing, word), item)| {
                self.checked_result(
                    ApiWord {
                        id: item.id,
                        external_id: item.external_id.unwrap_or_default(),
                        checksum: item.checksum,
                    },
                    word,
                    Some(&existing.id),
                )
                .map_err(mark_batch_integrity_error)
            })
            .collect()
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

impl ExamplesApi for CheatsheetApi {
    fn find_all_examples_by_external_id(&self) -> Result<HashMap<String, ApiExample>> {
        self.find_all_examples()
    }

    fn create_examples_batch(
        &self,
        examples: &[ImportedExample],
    ) -> std::result::Result<Vec<ApiExample>, ApiWriteError> {
        if examples.is_empty() {
            return Ok(Vec::new());
        }

        let requests: Vec<_> = examples
            .iter()
            .map(|example| example.request(true))
            .collect();
        let response = self.send_batch_request(
            Method::POST,
            self.base_url
                .join("examples/batch")
                .map_err(|error| api_error(error.into()))?,
            &requests,
            "examples",
            true,
        )?;
        let items: Vec<ExampleResponse> = response.json().map_err(|error| ApiWriteError {
            message: format!("API returned an unreadable examples batch response: {error}"),
            outcome_unknown: true,
            fatal: false,
        })?;
        self.validate_batch_response_count("examples", examples.len(), items.len())?;

        examples
            .iter()
            .zip(items)
            .map(|(example, item)| {
                let Some(external_id) = item.external_id else {
                    return Err(ApiWriteError {
                        message: "API returned an example without externalId".to_owned(),
                        outcome_unknown: true,
                        fatal: true,
                    });
                };
                self.checked_example_result(
                    ApiExample {
                        id: item.id,
                        external_id,
                        checksum: item.checksum,
                    },
                    example,
                    None,
                )
                .map_err(mark_batch_integrity_error)
            })
            .collect()
    }

    fn update_examples_batch(
        &self,
        updates: &[(ApiExample, ImportedExample)],
    ) -> std::result::Result<Vec<ApiExample>, ApiWriteError> {
        if updates.is_empty() {
            return Ok(Vec::new());
        }

        let requests: Vec<_> = updates
            .iter()
            .map(|(existing, example)| example.update_request(&existing.id))
            .collect();
        let response = self.send_batch_request(
            Method::PUT,
            self.base_url
                .join("examples/batch")
                .map_err(|error| api_error(error.into()))?,
            &requests,
            "examples",
            true,
        )?;
        let items: Vec<ExampleResponse> = response.json().map_err(|error| ApiWriteError {
            message: format!("API returned an unreadable examples batch response: {error}"),
            outcome_unknown: true,
            fatal: false,
        })?;
        self.validate_batch_response_count("examples", updates.len(), items.len())?;

        updates
            .iter()
            .zip(items)
            .map(|((existing, example), item)| {
                let Some(external_id) = item.external_id else {
                    return Err(ApiWriteError {
                        message: "API returned an example without externalId".to_owned(),
                        outcome_unknown: true,
                        fatal: true,
                    });
                };
                self.checked_example_result(
                    ApiExample {
                        id: item.id,
                        external_id,
                        checksum: item.checksum,
                    },
                    example,
                    Some(&existing.id),
                )
                .map_err(mark_batch_integrity_error)
            })
            .collect()
    }

    fn create_example(
        &self,
        example: &ImportedExample,
    ) -> std::result::Result<ExampleWriteResult, ApiWriteError> {
        match self.send_example(
            Method::POST,
            self.example_url(None).map_err(api_error)?,
            example,
            true,
        ) {
            Ok(api_example) => {
                let api_example = self.checked_example_result(api_example, example, None)?;
                Ok(ExampleWriteResult {
                    example: api_example,
                    effect: WriteEffect::Created,
                })
            }
            Err(error) => Err(error),
        }
    }

    fn update_example(
        &self,
        existing: &ApiExample,
        example: &ImportedExample,
    ) -> std::result::Result<ExampleWriteResult, ApiWriteError> {
        let url = self.example_url(Some(&existing.id)).map_err(api_error)?;
        match self.send_example(Method::PUT, url, example, false) {
            Ok(api_example) => {
                let api_example =
                    self.checked_example_result(api_example, example, Some(&existing.id))?;
                Ok(ExampleWriteResult {
                    example: api_example,
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

fn mark_batch_integrity_error(mut error: ApiWriteError) -> ApiWriteError {
    error.outcome_unknown = true;
    error
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExamplePage {
    data: Vec<ExampleReference>,
    has_next_page: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExampleReference {
    id: String,
    external_id: Option<String>,
    checksum: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExampleResponse {
    id: String,
    external_id: Option<String>,
    checksum: Option<String>,
}
