use crate::domain::word::ImportedWord;
use anyhow::Result;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};

pub struct SourceSnapshot {
    pub words: Vec<SourceWord>,
    pub translations_by_word_id: HashMap<String, Vec<SourceTranslation>>,
    pub aspects_by_word_id: HashMap<String, Option<String>>,
    pub forms_by_word_id: HashMap<String, Vec<SourceForm>>,
}

pub struct SourceWord {
    pub id: String,
    pub russian: String,
    pub source_type: Option<String>,
    pub disabled: bool,
    pub audio_url: Option<String>,
    pub usage: Option<String>,
}

pub struct SourceTranslation {
    pub id: i64,
    pub position: Option<i64>,
    pub text: String,
}

pub struct SourceForm {
    pub id: i64,
    pub position: Option<i64>,
    pub form_type: String,
    pub form_bare: String,
}

#[derive(Clone, Debug)]
pub struct ApiWord {
    pub id: String,
    pub external_id: String,
    pub checksum: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub enum WriteEffect {
    Created,
    Updated,
}

pub struct WriteResult {
    pub word: ApiWord,
    pub effect: WriteEffect,
}

#[derive(Debug)]
pub struct ApiWriteError {
    pub message: String,
    pub outcome_unknown: bool,
    pub fatal: bool,
}

impl Display for ApiWriteError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ApiWriteError {}

pub trait WordSource {
    fn load_snapshot(&self) -> Result<SourceSnapshot>;
}

pub trait WordsApi {
    fn find_all_by_external_id(&self) -> Result<HashMap<String, ApiWord>>;

    fn create(&self, word: &ImportedWord) -> std::result::Result<WriteResult, ApiWriteError>;

    fn update(
        &self,
        existing: &ApiWord,
        word: &ImportedWord,
    ) -> std::result::Result<WriteResult, ApiWriteError>;
}
