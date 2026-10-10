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

#[derive(Default)]
pub struct SourceWordRelationSnapshot {
    pub relations: Vec<SourceWordRelation>,
}

pub struct SourceWordRelation {
    pub word_id: String,
    pub related_word_id: String,
    pub relation: String,
}

#[derive(Clone, Debug)]
pub struct ApiWord {
    pub id: String,
    pub external_id: String,
    pub checksum: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ApiExample {
    pub id: String,
    pub external_id: String,
    pub checksum: Option<String>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct WordRelationKey {
    pub source_external_id: String,
    pub related_external_id: String,
    pub relation: String,
}

#[derive(Clone, Debug)]
pub struct ApiWordRelationCreate {
    pub key: WordRelationKey,
    pub word_id: String,
    pub related_word_id: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ApiWordRelation {
    pub source_word_id: String,
    pub source_external_id: Option<String>,
    pub related_word_id: String,
    pub related_external_id: Option<String>,
    pub relation: String,
}

impl ApiWordRelation {
    pub fn source_key(&self) -> Option<WordRelationKey> {
        Some(WordRelationKey {
            source_external_id: self.source_external_id.clone()?,
            related_external_id: self.related_external_id.clone()?,
            relation: self.relation.clone(),
        })
    }

    pub fn api_identity(&self) -> (String, String, String) {
        (
            self.source_word_id.clone(),
            self.related_word_id.clone(),
            self.relation.clone(),
        )
    }
}

pub struct ApiWordRelationResult {
    pub key: WordRelationKey,
    pub created: bool,
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

    fn load_word_relation_snapshot(&self) -> Result<SourceWordRelationSnapshot> {
        Ok(SourceWordRelationSnapshot::default())
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

    fn delete_words_batch(&self, words: &[ApiWord]) -> std::result::Result<(), ApiWriteError>;
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

    fn delete_examples_batch(
        &self,
        examples: &[ApiExample],
    ) -> std::result::Result<(), ApiWriteError>;
}

pub trait WordRelationsApi {
    fn find_all_word_relations(&self) -> Result<Vec<ApiWordRelation>>;

    fn create_word_relations_batch(
        &self,
        relations: &[ApiWordRelationCreate],
    ) -> std::result::Result<Vec<ApiWordRelationResult>, ApiWriteError>;

    fn delete_word_relations_batch(
        &self,
        relations: &[ApiWordRelation],
    ) -> std::result::Result<(), ApiWriteError>;
}
