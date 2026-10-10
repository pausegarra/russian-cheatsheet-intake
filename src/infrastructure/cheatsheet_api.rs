use crate::application::ports::{
    ApiExample, ApiWord, ApiWordRelation, ApiWordRelationCreate, ApiWordRelationResult,
    ApiWriteError, ExamplesApi, WordRelationsApi, WordsApi,
};
use crate::domain::example::ImportedExample;
use crate::domain::word::ImportedWord;
use crate::infrastructure::keycloak::JwtKeycloakContextHolder;
use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};
use url::Url;

const PAGE_SIZE: usize = 1000;
const RELATION_PAGE_SIZE: usize = 1000;
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
            let mut url = self.base_url.join("v1/words")?;
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
            let mut url = self.base_url.join("v1/examples")?;
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

    fn find_all_word_relations(&self) -> Result<Vec<ApiWordRelation>> {
        let mut relations = Vec::new();
        let mut identities = HashSet::new();
        let mut page = 0usize;
        info!("Starting Russian Cheatsheet API word relation read");

        loop {
            debug!(
                page,
                page_size = RELATION_PAGE_SIZE,
                "Requesting API word relation page"
            );
            let mut url = self.base_url.join("v1/words/relations")?;
            url.query_pairs_mut()
                .append_pair("page", &page.to_string())
                .append_pair("perPage", &RELATION_PAGE_SIZE.to_string());
            let token = self.keycloak.get_token()?;

            let response: WordRelationPage = self
                .http
                .get(url)
                .bearer_auth(token)
                .send()
                .with_context(|| {
                    format!("failed to read Russian Cheatsheet API word relations page {page}")
                })?
                .error_for_status()
                .with_context(|| {
                    format!("Russian Cheatsheet API rejected word relations page {page}")
                })?
                .json()
                .with_context(|| {
                    format!("invalid Russian Cheatsheet API response on relations page {page}")
                })?;

            let page_relation_count = response.data.len();
            let has_next_page = response.has_next_page;
            for item in response.data {
                let relation = ApiWordRelation {
                    source_word_id: item.source_word_id,
                    source_external_id: item.source_external_id,
                    related_word_id: item.related_word_id,
                    related_external_id: item.related_external_id,
                    relation: item.relation,
                };
                if !identities.insert(relation.api_identity()) {
                    bail!(
                        "Russian Cheatsheet API returned duplicate word relation {} -> {} ({})",
                        relation.source_word_id,
                        relation.related_word_id,
                        relation.relation
                    );
                }
                relations.push(relation);
            }

            info!(
                page,
                word_relations = page_relation_count,
                has_next_page,
                "Russian Cheatsheet API word relation page received"
            );
            if !has_next_page {
                break;
            }
            page += 1;
        }

        info!(
            word_relations = relations.len(),
            "Russian Cheatsheet API word relation read complete"
        );
        Ok(relations)
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
                .join("v1/words/batch")
                .map_err(|error| api_error(error.into()))?,
            &requests,
            "words",
            true,
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
                .join("v1/words/batch")
                .map_err(|error| api_error(error.into()))?,
            &requests,
            "words",
            true,
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

    fn delete_words_batch(&self, words: &[ApiWord]) -> std::result::Result<(), ApiWriteError> {
        if words.is_empty() {
            return Ok(());
        }

        let ids = words
            .iter()
            .map(|word| word.id.as_str())
            .collect::<Vec<_>>();
        self.send_batch_request(
            Method::DELETE,
            self.base_url
                .join("v1/words/batch")
                .map_err(|error| api_error(error.into()))?,
            &ids,
            "words",
            true,
        )?;
        Ok(())
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
                .join("v1/examples/batch")
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
                .join("v1/examples/batch")
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

    fn delete_examples_batch(
        &self,
        examples: &[ApiExample],
    ) -> std::result::Result<(), ApiWriteError> {
        if examples.is_empty() {
            return Ok(());
        }

        let ids = examples
            .iter()
            .map(|example| example.id.as_str())
            .collect::<Vec<_>>();
        self.send_batch_request(
            Method::DELETE,
            self.base_url
                .join("v1/examples/batch")
                .map_err(|error| api_error(error.into()))?,
            &ids,
            "examples",
            true,
        )?;
        Ok(())
    }
}

impl WordRelationsApi for CheatsheetApi {
    fn find_all_word_relations(&self) -> Result<Vec<ApiWordRelation>> {
        CheatsheetApi::find_all_word_relations(self)
    }

    fn create_word_relations_batch(
        &self,
        relations: &[ApiWordRelationCreate],
    ) -> std::result::Result<Vec<ApiWordRelationResult>, ApiWriteError> {
        if relations.is_empty() {
            return Ok(Vec::new());
        }

        let requests = relations
            .iter()
            .map(|relation| WordRelationRequest {
                word_id: &relation.word_id,
                related_word_id: &relation.related_word_id,
                relation: &relation.key.relation,
            })
            .collect::<Vec<_>>();
        let response = self.send_batch_request(
            Method::POST,
            self.base_url
                .join("v1/words/relations/batch")
                .map_err(|error| api_error(error.into()))?,
            &requests,
            "word relations",
            true,
        )?;
        let items: Vec<WordRelationBatchResponse> =
            response.json().map_err(|error| ApiWriteError {
                message: format!(
                    "API returned an unreadable word relations batch response: {error}"
                ),
                outcome_unknown: true,
                fatal: false,
            })?;
        if items.len() != relations.len() {
            return Err(mark_batch_integrity_error(ApiWriteError {
                message: format!(
                    "API returned {} word relations for batch of {}",
                    items.len(),
                    relations.len()
                ),
                outcome_unknown: true,
                fatal: true,
            }));
        }

        relations
            .iter()
            .zip(items)
            .map(|(requested, item)| {
                if item.word_id != requested.word_id
                    || item.relation.related_word_id != requested.related_word_id
                    || item.relation.relation != requested.key.relation
                {
                    return Err(mark_batch_integrity_error(ApiWriteError {
                        message: format!(
                            "API returned word relation {} -> {} ({}) for requested {} -> {} ({})",
                            item.word_id,
                            item.relation.related_word_id,
                            item.relation.relation,
                            requested.word_id,
                            requested.related_word_id,
                            requested.key.relation
                        ),
                        outcome_unknown: true,
                        fatal: true,
                    }));
                }
                if item.relation.checksum.as_deref().is_none_or(str::is_empty) {
                    return Err(mark_batch_integrity_error(ApiWriteError {
                        message: format!(
                            "API returned no checksum for word relation {} -> {} ({})",
                            requested.word_id, requested.related_word_id, requested.key.relation
                        ),
                        outcome_unknown: true,
                        fatal: true,
                    }));
                }

                Ok(ApiWordRelationResult {
                    key: requested.key.clone(),
                    created: item.created,
                })
            })
            .collect()
    }

    fn delete_word_relations_batch(
        &self,
        relations: &[ApiWordRelation],
    ) -> std::result::Result<(), ApiWriteError> {
        if relations.is_empty() {
            return Ok(());
        }

        let requests = relations
            .iter()
            .map(|relation| WordRelationRequest {
                word_id: &relation.source_word_id,
                related_word_id: &relation.related_word_id,
                relation: &relation.relation,
            })
            .collect::<Vec<_>>();
        self.send_batch_request(
            Method::DELETE,
            self.base_url
                .join("v1/words/relations/batch")
                .map_err(|error| api_error(error.into()))?,
            &requests,
            "word relations",
            true,
        )?;
        Ok(())
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WordRelationRequest<'a> {
    word_id: &'a str,
    related_word_id: &'a str,
    relation: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WordRelationPage {
    data: Vec<WordRelationReference>,
    has_next_page: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WordRelationReference {
    source_word_id: String,
    source_external_id: Option<String>,
    related_word_id: String,
    related_external_id: Option<String>,
    relation: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WordRelationBatchResponse {
    word_id: String,
    created: bool,
    relation: WordRelationResult,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WordRelationResult {
    related_word_id: String,
    relation: String,
    checksum: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::WordRelationReference;

    #[test]
    fn keeps_relation_uuids_when_endpoint_external_ids_are_missing() {
        let relation: WordRelationReference = serde_json::from_str(
            r#"{
              "sourceWordId":"manual-source-uuid",
              "sourceExternalId":null,
              "relatedWordId":"imported-target-uuid",
              "relatedExternalId":"openrussian-target",
              "relation":"synonym"
            }"#,
        )
        .expect("API relation row deserializes");

        assert_eq!(relation.source_word_id, "manual-source-uuid");
        assert_eq!(relation.source_external_id, None);
        assert_eq!(relation.related_word_id, "imported-target-uuid");
        assert_eq!(
            relation.related_external_id.as_deref(),
            Some("openrussian-target")
        );
        assert_eq!(relation.relation, "synonym");
    }
}
