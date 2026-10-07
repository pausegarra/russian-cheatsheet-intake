use anyhow::Result;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExampleTranslation {
    pub language: String,
    pub text: String,
    pub position: i32,
}

#[derive(Clone, Debug)]
pub struct ImportedExample {
    pub external_id: String,
    pub russian: String,
    pub translations: Vec<ExampleTranslation>,
    pub contributor: Option<String>,
    pub audio_url: Option<String>,
    pub linked_word_ids: Vec<String>,
    pub checksum: String,
}

impl ImportedExample {
    pub fn new(
        external_id: String,
        russian: String,
        translations: Vec<ExampleTranslation>,
        contributor: Option<String>,
        audio_url: Option<String>,
        mut linked_word_ids: Vec<String>,
    ) -> Result<Self> {
        crate::domain::example_checksum::sort_linked_word_ids(&mut linked_word_ids);
        let mut example = Self {
            external_id,
            russian,
            translations,
            contributor,
            audio_url,
            linked_word_ids,
            checksum: String::new(),
        };
        example.checksum = crate::domain::example_checksum::calculate(&example)?;
        Ok(example)
    }

    pub fn request(&self, include_external_id: bool) -> ExampleWriteRequest<'_> {
        ExampleWriteRequest {
            external_id: include_external_id.then_some(self.external_id.as_str()),
            russian: &self.russian,
            translations: &self.translations,
            contributor: self.contributor.as_deref(),
            audio_url: self.audio_url.as_deref(),
            linked_word_ids: &self.linked_word_ids,
        }
    }

    pub fn update_request<'a>(&'a self, id: &'a str) -> ExampleUpdateRequest<'a> {
        ExampleUpdateRequest {
            id,
            example: self.request(false),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExampleWriteRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_id: Option<&'a str>,
    pub russian: &'a str,
    pub translations: &'a [ExampleTranslation],
    pub contributor: Option<&'a str>,
    pub audio_url: Option<&'a str>,
    pub linked_word_ids: &'a [String],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExampleUpdateRequest<'a> {
    pub id: &'a str,
    #[serde(flatten)]
    pub example: ExampleWriteRequest<'a>,
}
