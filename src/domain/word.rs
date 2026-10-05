use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;

pub type WordForms = BTreeMap<String, Option<String>>;

pub const WORD_FORM_TYPES: &[&str] = &[
    "ru_base",
    "ru_noun_sg_nom",
    "ru_noun_sg_gen",
    "ru_noun_sg_dat",
    "ru_noun_sg_acc",
    "ru_noun_sg_inst",
    "ru_noun_sg_prep",
    "ru_noun_pl_nom",
    "ru_noun_pl_gen",
    "ru_noun_pl_dat",
    "ru_noun_pl_acc",
    "ru_noun_pl_inst",
    "ru_noun_pl_prep",
    "ru_verb_imperative_sg",
    "ru_verb_imperative_pl",
    "ru_verb_past_m",
    "ru_verb_past_f",
    "ru_verb_past_n",
    "ru_verb_past_pl",
    "ru_verb_presfut_sg1",
    "ru_verb_presfut_sg2",
    "ru_verb_presfut_sg3",
    "ru_verb_presfut_pl1",
    "ru_verb_presfut_pl2",
    "ru_verb_presfut_pl3",
    "ru_verb_gerund_present",
    "ru_verb_gerund_past",
    "ru_verb_participle_active_present",
    "ru_verb_participle_active_past",
    "ru_verb_participle_passive_present",
    "ru_verb_participle_passive_past",
    "ru_adj_comparative",
    "ru_adj_superlative",
    "ru_adj_short_m",
    "ru_adj_short_f",
    "ru_adj_short_n",
    "ru_adj_short_pl",
    "ru_adj_m_nom",
    "ru_adj_m_gen",
    "ru_adj_m_dat",
    "ru_adj_m_acc",
    "ru_adj_m_inst",
    "ru_adj_m_prep",
    "ru_adj_f_nom",
    "ru_adj_f_gen",
    "ru_adj_f_dat",
    "ru_adj_f_acc",
    "ru_adj_f_inst",
    "ru_adj_f_prep",
    "ru_adj_n_nom",
    "ru_adj_n_gen",
    "ru_adj_n_dat",
    "ru_adj_n_acc",
    "ru_adj_n_inst",
    "ru_adj_n_prep",
    "ru_adj_pl_nom",
    "ru_adj_pl_gen",
    "ru_adj_pl_dat",
    "ru_adj_pl_acc",
    "ru_adj_pl_inst",
    "ru_adj_pl_prep",
];

#[derive(Clone, Debug)]
pub struct Translation {
    pub language: String,
    pub text: String,
    pub position: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct ImportedWord {
    pub external_id: String,
    pub russian: String,
    pub word_type: Option<String>,
    pub aspect: Option<String>,
    pub translations: Vec<Translation>,
    pub usage: Option<String>,
    pub audio_url: Option<String>,
    pub forms: Option<WordForms>,
    pub checksum: String,
}

impl ImportedWord {
    pub fn new(
        external_id: String,
        russian: String,
        word_type: Option<String>,
        aspect: Option<String>,
        translations: Vec<Translation>,
        usage: Option<String>,
        audio_url: Option<String>,
        forms: Option<WordForms>,
    ) -> Result<Self> {
        let mut word = Self {
            external_id,
            russian,
            word_type,
            aspect,
            translations,
            usage,
            audio_url,
            forms,
            checksum: String::new(),
        };
        word.checksum = crate::domain::checksum::calculate(&word)?;
        Ok(word)
    }

    pub fn request(&self) -> WordWriteRequest<'_> {
        WordWriteRequest {
            russian: &self.russian,
            word_type: self.word_type.as_deref(),
            aspect: self.aspect.as_deref(),
            forms: self.forms.as_ref(),
            external_id: &self.external_id,
            translations: self
                .translations
                .iter()
                .map(TranslationRequest::from)
                .collect(),
            usage: self.usage.as_deref(),
            audio_url: self.audio_url.as_deref(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WordWriteRequest<'a> {
    pub russian: &'a str,
    #[serde(rename = "type")]
    pub word_type: Option<&'a str>,
    pub aspect: Option<&'a str>,
    pub forms: Option<&'a WordForms>,
    pub external_id: &'a str,
    pub translations: Vec<TranslationRequest<'a>>,
    pub usage: Option<&'a str>,
    pub audio_url: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationRequest<'a> {
    pub language: &'a str,
    pub text: &'a str,
    pub position: Option<i64>,
}

impl<'a> From<&'a Translation> for TranslationRequest<'a> {
    fn from(translation: &'a Translation) -> Self {
        Self {
            language: &translation.language,
            text: &translation.text,
            position: translation.position,
        }
    }
}
