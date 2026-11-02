use anyhow::{Context, Result};
use reqwest::blocking::Client;
use roxmltree::{Document, Node, NodeType, ParsingOptions};
use std::collections::HashMap;

const FREEDICT_TEI_URL: &str = "https://download.freedict.org/generated/rus-spa/rus-spa.tei";
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

pub(super) fn load(client: &Client) -> Result<HashMap<String, Vec<String>>> {
    let tei = client
        .get(FREEDICT_TEI_URL)
        .send()
        .context("failed to download FreeDict Russian-Spanish dictionary")?
        .error_for_status()
        .context("FreeDict rejected Russian-Spanish dictionary download")?
        .text()
        .context("failed to read FreeDict Russian-Spanish dictionary")?;
    // FreeDict declares an external DTD; roxmltree parses its identifier without fetching it.
    let document = Document::parse_with_options(
        &tei,
        ParsingOptions {
            allow_dtd: true,
            ..ParsingOptions::default()
        },
    )
    .context("invalid FreeDict TEI document")?;
    let mut translations_by_word = HashMap::new();

    for entry in document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "entry")
    {
        let spanish_translations = entry
            .descendants()
            .filter(|node| {
                node.is_element()
                    && node.tag_name().name() == "cit"
                    && node.attribute("type") == Some("trans")
                    && node.attribute((XML_NAMESPACE, "lang")) == Some("es")
            })
            .flat_map(spanish_quotes)
            .collect::<Vec<_>>();

        if spanish_translations.is_empty() {
            continue;
        }

        for orthography in entry
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "orth")
            .filter_map(|node| node.text())
        {
            let key = super::lookup_key(orthography);
            if key.is_empty() {
                continue;
            }
            let existing = translations_by_word.entry(key).or_insert_with(Vec::new);
            for translation in &spanish_translations {
                super::push_unique(existing, translation);
            }
        }
    }

    Ok(translations_by_word)
}

fn spanish_quotes(citation: Node<'_, '_>) -> Vec<String> {
    citation
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "quote")
        .filter_map(quote_text)
        .collect()
}

fn quote_text(quote: Node<'_, '_>) -> Option<String> {
    let text = quote
        .descendants()
        // Node::text() on elements returns their first text child too.
        .filter(|node| node.node_type() == NodeType::Text)
        .filter_map(|node| node.text())
        .collect::<String>();
    let text = text.replace("<u>", "").replace("</u>", "");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}
