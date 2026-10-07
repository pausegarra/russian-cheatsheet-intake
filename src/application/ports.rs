use crate::domain::example::ImportedExample;
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

#[derive(Default)]
pub struct SourceExampleSnapshot {
    pub examples: Vec<SourceExample>,
    pub translations_by_example_id: HashMap<String, Vec<SourceExampleTranslation>>,
    pub word_ids_by_example_id: HashMap<String, Vec<String>>,
}

pub struct SourceExample {
    pub id: String,
    pub russian: String,
    pub disabled: bool,
}

pub struct SourceExampleTranslation {
    pub language: String,
    pub text: String,
    pub position: i32,
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

#[derive(Clone, Debug)]
pub struct ApiExample {
    pub id: String,
    pub external_id: String,
    pub checksum: Option<String>,
}

pub struct ExampleWriteResult {
    pub example: ApiExample,
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

    fn load_example_snapshot(&self) -> Result<SourceExampleSnapshot> {
        Ok(SourceExampleSnapshot::default())
    }
}

pub trait WordsApi {
    fn find_all_by_external_id(&self) -> Result<HashMap<String, ApiWord>>;

    fn create_batch(
        &self,
        words: &[ImportedWord],
    ) -> std::result::Result<Vec<ApiWord>, ApiWriteError>;

    fn update_batch(
        &self,
        updates: &[(ApiWord, ImportedWord)],
    ) -> std::result::Result<Vec<ApiWord>, ApiWriteError>;

    fn create(&self, word: &ImportedWord) -> std::result::Result<WriteResult, ApiWriteError>;

    fn update(
        &self,
        existing: &ApiWord,
        word: &ImportedWord,
    ) -> std::result::Result<WriteResult, ApiWriteError>;
}

pub trait ExamplesApi {
    fn find_all_examples_by_external_id(&self) -> Result<HashMap<String, ApiExample>>;

    fn create_examples_batch(
        &self,
        examples: &[ImportedExample],
    ) -> std::result::Result<Vec<ApiExample>, ApiWriteError>;

    fn update_examples_batch(
        &self,
        updates: &[(ApiExample, ImportedExample)],
    ) -> std::result::Result<Vec<ApiExample>, ApiWriteError>;

    fn create_example(
        &self,
        example: &ImportedExample,
    ) -> std::result::Result<ExampleWriteResult, ApiWriteError>;

    fn update_example(
        &self,
        existing: &ApiExample,
        example: &ImportedExample,
    ) -> std::result::Result<ExampleWriteResult, ApiWriteError>;
}
