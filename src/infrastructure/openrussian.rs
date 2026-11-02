use crate::application::ports::{
    SourceExample, SourceExampleSnapshot, SourceExampleTranslation, SourceForm, SourceSnapshot,
    SourceTranslation, SourceWord, SourceWordRelation, SourceWordRelationSnapshot, WordSource,
};
use anyhow::{Context, Result, bail};
use csv::ReaderBuilder;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info};
use url::Url;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CSV_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const TOGETHERDB_MGMT_URL: &str = "https://mgmt.togetherdb.com";

pub struct OpenRussianClient {
    http: Client,
    worker_url: Url,
    connection_id: String,
    database_name: String,
}

impl OpenRussianClient {
    pub fn from_share_url(share_url: &str) -> Result<Self> {
        let share_url = Url::parse(share_url).context("invalid OpenRussian TogetherDB URL")?;
        if share_url.scheme() != "https" || share_url.host_str() != Some("app.togetherdb.com") {
            bail!("OpenRussian URL must use https://app.togetherdb.com");
        }

        let path: Vec<&str> = share_url
            .path_segments()
            .context("TogetherDB URL has no path")?
            .collect();
        if path.len() < 4 || path[0] != "db" {
            bail!("TogetherDB URL must use /db/<connectionId>/<databaseName>/<tableName>");
        }

        let connection_id = path[1].to_owned();
        let database_name = path[2].to_owned();
        if connection_id.is_empty() || database_name.is_empty() {
            bail!("TogetherDB URL must include connection ID and database name");
        }

        let http = Client::builder()
            .user_agent(concat!(
                "russian-cheatsheet-intake/",
                env!("CARGO_PKG_VERSION")
            ))
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("could not create TogetherDB HTTP client")?;
        let worker_url = resolve_worker(&http, &connection_id)?;

        Ok(Self {
            http,
            worker_url,
            connection_id,
            database_name,
        })
    }

    fn fetch_table(&self, table_name: &str, filter: Option<&str>) -> Result<Vec<TableRow>> {
        info!(table = table_name, "Starting OpenRussian table CSV export");
        let mut export_url = endpoint(
            &self.worker_url,
            &[
                "connections",
                &self.connection_id,
                "databases",
                &self.database_name,
                "tables",
                table_name,
                "export",
            ],
        )?;
        {
            let mut query = export_url.query_pairs_mut();
            query
                .append_pair("expand", "false")
                .append_pair("filter", filter.unwrap_or(""))
                .append_pair("separator", ",");
        }

        debug!(table = table_name, filter = ?filter, "Requesting OpenRussian CSV export");
        let export: ExportEnvelope = self
            .http
            .post(export_url)
            .send()
            .with_context(|| format!("failed to request OpenRussian table '{table_name}' export"))?
            .error_for_status()
            .with_context(|| format!("TogetherDB rejected table '{table_name}' export"))?
            .json()
            .with_context(|| {
                format!("invalid TogetherDB export response for table '{table_name}'")
            })?;

        let export_url = endpoint(&self.worker_url, &["exports", &export.result.export_key])?;
        let csv = self
            .http
            .get(export_url)
            .timeout(CSV_DOWNLOAD_TIMEOUT)
            .send()
            .with_context(|| format!("failed to download OpenRussian table '{table_name}' CSV"))?
            .error_for_status()
            .with_context(|| format!("TogetherDB rejected table '{table_name}' CSV download"))?
            .bytes()
            .with_context(|| {
                format!("failed to read OpenRussian table '{table_name}' CSV bytes")
            })?;
        let rows = parse_table_csv(table_name, csv.as_ref())?;

        info!(
            table = table_name,
            rows = rows.len(),
            "OpenRussian table CSV loaded"
        );
        Ok(rows)
    }
}

impl WordSource for OpenRussianClient {
    fn load_snapshot(&self) -> Result<SourceSnapshot> {
        let words = self.fetch_table("words", None)?;
        let translations = self.fetch_table("translations", Some("lang=en"))?;
        let verbs = self.fetch_table("verbs", None)?;
        let word_forms = self.fetch_table("words_forms", None)?;

        let mut source_words = Vec::with_capacity(words.len());
        for row in words {
            source_words.push(SourceWord {
                id: row.string("words", "id")?,
                russian: row.string("words", "bare")?,
                russian_accented: row
                    .optional_string("words", "accented")?
                    .filter(|value| !value.trim().is_empty()),
                source_type: row.optional_string("words", "type")?,
                disabled: row.boolean("words", "disabled")?,
                audio_url: row.optional_string("words", "audio")?,
                usage: row.optional_string("words", "usage_en")?,
            });
        }

        let mut translations_by_word_id: HashMap<String, Vec<SourceTranslation>> = HashMap::new();
        for row in translations {
            let word_id = row.string("translations", "word_id")?;
            translations_by_word_id
                .entry(word_id)
                .or_default()
                .push(SourceTranslation {
                    id: row.integer("translations", "id")?,
                    position: row.optional_integer("translations", "position")?,
                    text: row.string("translations", "tl")?,
                });
        }

        let mut aspects_by_word_id = HashMap::new();
        for row in verbs {
            let word_id = row.string("verbs", "word_id")?;
            let aspect = row.optional_string("verbs", "aspect")?.and_then(|value| {
                let aspect = value.trim().to_owned();
                (!aspect.is_empty()).then_some(aspect)
            });
            if let Some(previous) = aspects_by_word_id.insert(word_id.clone(), aspect.clone()) {
                if previous != aspect {
                    bail!("OpenRussian verb {word_id} has conflicting aspects");
                }
            }
        }

        let mut forms_by_word_id: HashMap<String, Vec<SourceForm>> = HashMap::new();
        for row in word_forms {
            let word_id = row.string("words_forms", "word_id")?;
            forms_by_word_id
                .entry(word_id)
                .or_default()
                .push(SourceForm {
                    id: row.integer("words_forms", "id")?,
                    position: row.optional_integer("words_forms", "position")?,
                    form_type: row.string("words_forms", "form_type")?,
                    form_bare: row.string("words_forms", "form_bare")?,
                });
        }

        Ok(SourceSnapshot {
            words: source_words,
            translations_by_word_id,
            aspects_by_word_id,
            forms_by_word_id,
        })
    }

    fn load_example_snapshot(&self) -> Result<SourceExampleSnapshot> {
        let sentences = self.fetch_table("sentences", None)?;
        let sentence_translations = self.fetch_table("sentences_translations", None)?;
        let sentence_words = self.fetch_table("sentences_words", None)?;

        let mut examples = Vec::with_capacity(sentences.len());
        for row in sentences {
            examples.push(SourceExample {
                id: row.string("sentences", "id")?,
                russian: row.string("sentences", "ru")?,
                disabled: row.boolean("sentences", "disabled")?,
            });
        }

        let mut translations_by_example_id: HashMap<String, Vec<SourceExampleTranslation>> =
            HashMap::new();
        for row in sentence_translations {
            let sentence_id = row.string("sentences_translations", "sentence_id")?;
            let translations = translations_by_example_id.entry(sentence_id).or_default();
            for (language, column, position) in [("en", "tl_en", 0), ("de", "tl_de", 1)] {
                if let Some(text) = row
                    .optional_string("sentences_translations", column)?
                    .filter(|text| !text.trim().is_empty())
                {
                    translations.push(SourceExampleTranslation {
                        language: language.to_owned(),
                        text,
                        position,
                    });
                }
            }
        }

        let mut word_ids_by_example_id: HashMap<String, Vec<String>> = HashMap::new();
        for row in sentence_words {
            let sentence_id = row.string("sentences_words", "sentence_id")?;
            word_ids_by_example_id
                .entry(sentence_id)
                .or_default()
                .push(row.string("sentences_words", "word_id")?);
        }

        info!(
            source_examples = examples.len(),
            translated_sentences = translations_by_example_id.len(),
            sentence_word_links = word_ids_by_example_id.values().map(Vec::len).sum::<usize>(),
            "OpenRussian example data loaded"
        );
        Ok(SourceExampleSnapshot {
            examples,
            translations_by_example_id,
            word_ids_by_example_id,
        })
    }

    fn load_word_relation_snapshot(&self) -> Result<SourceWordRelationSnapshot> {
        let rows = self.fetch_table("words_rels", None)?;
        let mut relations = Vec::with_capacity(rows.len());
        for row in rows {
            relations.push(SourceWordRelation {
                word_id: row.string("words_rels", "word_id")?,
                related_word_id: row.string("words_rels", "rel_word_id")?,
                relation: row.string("words_rels", "relation")?,
            });
        }

        info!(
            source_word_relations = relations.len(),
            "OpenRussian word relations loaded"
        );
        Ok(SourceWordRelationSnapshot { relations })
    }
}

#[derive(Deserialize)]
struct WorkerEnvelope {
    result: WorkerInfo,
}

#[derive(Deserialize)]
struct WorkerInfo {
    hostname: String,
}

#[derive(Deserialize)]
struct ExportEnvelope {
    result: ExportResult,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportResult {
    export_key: String,
}

fn resolve_worker(http: &Client, connection_id: &str) -> Result<Url> {
    info!(connection_id, "Resolving OpenRussian TogetherDB worker");
    let mgmt_url = Url::parse(TOGETHERDB_MGMT_URL)?;
    let url = endpoint(&mgmt_url, &["connections", connection_id, "worker"])?;
    let response: WorkerEnvelope = http
        .get(url)
        .send()
        .context("failed to resolve OpenRussian TogetherDB worker")?
        .error_for_status()
        .context("TogetherDB rejected public connection lookup")?
        .json()
        .context("invalid TogetherDB worker response")?;

    let worker_url = Url::parse(&format!("https://{}", response.result.hostname))
        .context("TogetherDB returned an invalid worker hostname")?;
    if worker_url.scheme() != "https"
        || !worker_url
            .host_str()
            .is_some_and(|hostname| hostname.ends_with(".togetherdb.com"))
    {
        bail!("TogetherDB returned an unexpected worker host");
    }

    info!(
        worker_host = worker_url.host_str().unwrap_or("unknown"),
        "OpenRussian TogetherDB worker resolved"
    );
    Ok(worker_url)
}

fn endpoint(base: &Url, path: &[&str]) -> Result<Url> {
    let mut url = base.clone();
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("URL cannot accept path segments"))?;
        segments.clear();
        for segment in path {
            segments.push(segment);
        }
    }
    Ok(url)
}

struct TableRow {
    columns: Arc<Vec<String>>,
    values: Vec<Value>,
    source_row: usize,
}

fn parse_table_csv(table: &str, content: &[u8]) -> Result<Vec<TableRow>> {
    let mut reader = ReaderBuilder::new().from_reader(content);
    let columns = reader
        .headers()
        .with_context(|| format!("invalid CSV headers for OpenRussian table '{table}'"))?
        .iter()
        .enumerate()
        .map(|(index, header)| {
            if index == 0 {
                header.strip_prefix('\u{feff}').unwrap_or(header).to_owned()
            } else {
                header.to_owned()
            }
        })
        .collect::<Vec<_>>();
    let columns = Arc::new(columns);
    let mut rows = Vec::new();

    for (index, result) in reader.records().enumerate() {
        let record = result.with_context(|| {
            format!(
                "invalid CSV row {} for OpenRussian table '{table}'",
                index + 2
            )
        })?;
        let values = record
            .iter()
            .map(|value| Value::String(value.to_owned()))
            .collect();
        rows.push(TableRow {
            columns: Arc::clone(&columns),
            values,
            source_row: index + 2,
        });
    }

    Ok(rows)
}

impl TableRow {
    fn value(&self, table: &str, column: &str) -> Result<&Value> {
        let index = self
            .columns
            .iter()
            .position(|name| name == column)
            .with_context(|| format!("OpenRussian {table} has no '{column}' column"))?;
        self.values.get(index).with_context(|| {
            format!(
                "OpenRussian {table} row {} has no value for '{column}'",
                self.source_row
            )
        })
    }

    fn string(&self, table: &str, column: &str) -> Result<String> {
        let value = self.value(table, column)?;
        match value {
            Value::String(value) => Ok(value.clone()),
            Value::Number(value) => Ok(value.to_string()),
            _ => bail!(
                "OpenRussian {table} row {} has invalid '{column}' value",
                self.source_row
            ),
        }
    }

    fn optional_string(&self, table: &str, column: &str) -> Result<Option<String>> {
        match self.value(table, column)? {
            Value::Null => Ok(None),
            Value::String(value) if value.is_empty() => Ok(None),
            Value::String(value) => Ok(Some(value.clone())),
            Value::Number(value) => Ok(Some(value.to_string())),
            _ => bail!(
                "OpenRussian {table} row {} has invalid '{column}' value",
                self.source_row
            ),
        }
    }

    fn boolean(&self, table: &str, column: &str) -> Result<bool> {
        let value = self.value(table, column)?;
        match value {
            Value::Bool(value) => Ok(*value),
            Value::Number(value) => match value.as_i64() {
                Some(0) => Ok(false),
                Some(1) => Ok(true),
                _ => bail!(
                    "OpenRussian {table} row {} has invalid '{column}'",
                    self.source_row
                ),
            },
            Value::String(value) => match value.as_str() {
                "0" | "false" => Ok(false),
                "1" | "true" => Ok(true),
                _ => bail!(
                    "OpenRussian {table} row {} has invalid '{column}'",
                    self.source_row
                ),
            },
            _ => bail!(
                "OpenRussian {table} row {} has invalid '{column}'",
                self.source_row
            ),
        }
    }

    fn optional_integer(&self, table: &str, column: &str) -> Result<Option<i64>> {
        match self.value(table, column)? {
            Value::Null => Ok(None),
            Value::String(value) if value.is_empty() => Ok(None),
            Value::String(value) => value.parse::<i64>().map(Some).with_context(|| {
                format!(
                    "OpenRussian {table} row {} has invalid '{column}'",
                    self.source_row
                )
            }),
            Value::Number(value) => value.as_i64().map(Some).with_context(|| {
                format!(
                    "OpenRussian {table} row {} has invalid '{column}'",
                    self.source_row
                )
            }),
            _ => bail!(
                "OpenRussian {table} row {} has invalid '{column}'",
                self.source_row
            ),
        }
    }

    fn integer(&self, table: &str, column: &str) -> Result<i64> {
        match self.value(table, column)? {
            Value::Number(value) => value.as_i64().with_context(|| {
                format!(
                    "OpenRussian {table} row {} has invalid '{column}'",
                    self.source_row
                )
            }),
            Value::String(value) => value.parse::<i64>().with_context(|| {
                format!(
                    "OpenRussian {table} row {} has invalid '{column}'",
                    self.source_row
                )
            }),
            _ => bail!(
                "OpenRussian {table} row {} has invalid '{column}'",
                self.source_row
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::application::desired_state;
    use crate::application::ports::{
        SourceExampleSnapshot, SourceWordRelationSnapshot, WordSource,
    };
    use crate::application::synchronize::{
        PreparedWord, SyncSummary, prepare_words_with_spanish_translations,
    };
    use crate::domain::word::ImportedWord;
    use sha2::{Digest, Sha256};
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn snapshot_with_accent(accent: &str) -> crate::application::ports::SourceSnapshot {
        let csv = format!(
            "id,bare,accented,type,disabled,audio,usage_en\n1,привет,\"{accent}\",noun,0,,\n"
        );
        let listener = TcpListener::bind("127.0.0.1:0").expect("local fixture server binds");
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let tables = [
                ("words", csv.as_str()),
                ("translations", "id,word_id,position,tl\n"),
                ("verbs", "word_id,aspect\n"),
                ("words_forms", "word_id,id,position,form_type,form_bare\n"),
            ];
            for (table, csv) in tables {
                for export in [true, false] {
                    let (mut connection, _) = listener.accept().unwrap();
                    connection
                        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                        .unwrap();
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        let mut byte = [0];
                        connection.read_exact(&mut byte).unwrap();
                        request.push(byte[0]);
                    }
                    let request = String::from_utf8(request).unwrap();
                    let body = if export {
                        assert!(request.starts_with("POST "));
                        assert!(request.contains(&format!("/tables/{table}/export?")));
                        format!(r#"{{"result":{{"exportKey":"{table}"}}}}"#)
                    } else {
                        assert!(request.starts_with(&format!("GET /exports/{table} ")));
                        csv.to_owned()
                    };
                    write!(connection, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
            }
        });
        let client = super::OpenRussianClient {
            http: reqwest::blocking::Client::new(),
            worker_url: url::Url::parse(&format!("http://{address}")).unwrap(),
            connection_id: "test".to_owned(),
            database_name: "openrussian".to_owned(),
        };
        let snapshot = client.load_snapshot().expect("source CSV loads");
        server.join().expect("fixture server completes");
        snapshot
    }

    fn prepared_with_accent(accent: &str) -> ImportedWord {
        let mut words = prepare_words_with_spanish_translations(
            snapshot_with_accent(accent),
            &mut SyncSummary::default(),
            &HashMap::new(),
        );
        match words.pop().expect("one source word") {
            PreparedWord::Ready(word) => word,
            PreparedWord::Invalid { message, .. } => panic!("valid source word: {message}"),
        }
    }

    #[test]
    fn source_accent_survives_create_and_update_requests_exactly() {
        for accent in [
            "  приве'т  ",
            "приве\u{301}т",
            "й ё и\u{306} е\u{308}",
            "\u{1c}",
            "\u{1f}",
            "\u{feff}",
        ] {
            let word = prepared_with_accent(accent);
            let create = serde_json::to_value(word.request()).unwrap();
            let update = serde_json::to_value(word.update_request("api-word-id")).unwrap();
            assert_eq!(create["russian"], "привет");
            assert_eq!(create["externalId"], "1");
            assert_eq!(create["russianAccented"], accent);
            assert_eq!(update["russianAccented"], accent);
            assert_eq!(update["id"], "api-word-id");
        }
    }

    #[test]
    fn source_blank_accent_serializes_as_null() {
        for accent in ["", " \t\u{2003} ", "\u{a0}", "\u{85}", "\u{2007}", "\u{202f}"] {
            assert_eq!(snapshot_with_accent(accent).words[0].russian_accented, None);
            let word = prepared_with_accent(accent);
            let request = serde_json::to_value(word.request()).unwrap();
            let update = serde_json::to_value(word.update_request("api-word-id")).unwrap();
            assert_eq!(
                request.get("russianAccented"),
                Some(&serde_json::Value::Null)
            );
            assert_eq!(update["russianAccented"], serde_json::Value::Null);
            assert_eq!(word.checksum, prepared_with_accent("").checksum);
        }
    }

    #[test]
    fn accent_only_change_changes_checksum() {
        let baseline = prepared_with_accent("приве'т");
        let changed = prepared_with_accent("при'вет");
        let missing = prepared_with_accent("");
        assert_eq!(baseline.russian, changed.russian);
        assert_eq!(baseline.external_id, changed.external_id);
        assert_ne!(baseline.checksum, changed.checksum);
        assert_ne!(baseline.checksum, missing.checksum);
    }

    #[test]
    fn source_accent_survives_dry_run_with_bare_spanish_lookup() {
        let accent = "  приве'т\u{301}  ";
        let state = desired_state::build(
            snapshot_with_accent(accent),
            SourceWordRelationSnapshot::default(),
            SourceExampleSnapshot::default(),
            &HashMap::from([("привет".to_owned(), vec!["hola".to_owned()])]),
        )
        .expect("desired state builds");
        assert_eq!(state.words[0]["russianAccented"], accent);
        assert_eq!(state.words[0]["russian"], "привет");
        assert_eq!(state.words[0]["translations"][0]["text"], "hola");
        assert_eq!(state.summary.words_spanish_translated, 1);
        assert_eq!(state.pending_translations.len(), 0);
    }

    #[test]
    fn accented_checksum_matches_shared_api_fixture() {
        let fixture = include_bytes!("../../tests/fixtures/accented-word-checksum.json");
        let expected = "ede401cee0fb553c48cfdaf9cdd67e963c4de642247a431c86a20c598387e0c5";
        assert_eq!(format!("{:x}", Sha256::digest(fixture)), expected);
        assert_eq!(prepared_with_accent("приве'т").checksum, expected);
    }

    #[test]
    fn control_accent_checksum_matches_shared_api_fixture() {
        let fixture = include_bytes!("../../tests/fixtures/accented-word-control-checksum.json");
        let expected = "89014dd7dcb65e19bed20ca3244ad09856f26bcdb64d4280711a7832d4b2c14d";
        assert_eq!(format!("{:x}", Sha256::digest(fixture)), expected);
        assert_eq!(prepared_with_accent("\u{1c}").checksum, expected);
    }

    #[test]
    fn optional_accent_accepts_null_and_preserves_nonblank_strings() {
        let mut row = super::TableRow {
            columns: std::sync::Arc::new(vec!["accented".to_owned()]),
            values: vec![serde_json::Value::Null],
            source_row: 2,
        };
        assert_eq!(row.optional_string("words", "accented").unwrap(), None);
        let exact = "  приве'т\u{301}  ";
        row.values[0] = serde_json::Value::String(exact.to_owned());
        assert_eq!(
            row.optional_string("words", "accented").unwrap().as_deref(),
            Some(exact)
        );
    }

    #[test]
    fn allows_ten_minutes_for_large_csv_downloads() {
        assert_eq!(
            super::CSV_DOWNLOAD_TIMEOUT,
            std::time::Duration::from_secs(10 * 60)
        );
    }

    #[test]
    fn reads_export_key_from_togetherdb_response() {
        let response = r#"{"result":{"exportKey":"export-123"}}"#;
        let envelope: super::ExportEnvelope =
            serde_json::from_str(response).expect("export response deserializes");

        assert_eq!(envelope.result.export_key, "export-123");
    }

    #[test]
    fn parses_export_csv_with_bom_nulls_commas_and_newlines() {
        let csv = "\u{feff}id,word,usage_en,audio,position,disabled\n1,\"дом, улица\",\"строка 1\nстрока 2\",,,0\n";
        let rows = super::parse_table_csv("words", csv.as_bytes()).expect("CSV parses");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].integer("words", "id").unwrap(), 1);
        assert_eq!(rows[0].string("words", "word").unwrap(), "дом, улица");
        assert_eq!(rows[0].optional_string("words", "audio").unwrap(), None);
        assert_eq!(rows[0].optional_integer("words", "position").unwrap(), None);
        assert_eq!(
            rows[0].string("words", "usage_en").unwrap(),
            "строка 1\nстрока 2"
        );
        assert!(!rows[0].boolean("words", "disabled").unwrap());
    }
}
