//! Decode untrusted model output before invoking domain evidence rules.
use crate::domain::Source;
use crate::domain::evidence::{
    self, Claim, Evidence, Review, ReviewFormatError, Validated, passages,
};
use anyhow::{Context, Result};
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    claims: Vec<ClaimInput>,
    #[serde(default)]
    questions: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimInput {
    text: String,
    evidence: Vec<CitationInput>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CitationInput {
    source_id: i64,
    #[serde(default)]
    passage_id: Option<usize>,
    // Accept legacy exact quotes without weakening their validation.
    #[serde(default)]
    quote: Option<String>,
}

#[derive(Debug)]
pub struct DraftFormatError {
    // Diagnostic feedback goes back to the model as data, never into logs.
    pub detail: String,
}
impl std::fmt::Display for DraftFormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid claims format")
    }
}
impl std::error::Error for DraftFormatError {}

/// Tolerate only an outer Markdown transport wrapper, never extract arbitrary
/// JSON from surrounding prose or change claim text, quotes, IDs, or fields.
pub fn json_content(content: &str) -> &str {
    let trimmed = content.trim();
    for prefix in ["```json\n", "```\n", "```json\r\n", "```\r\n"] {
        if let Some(body) = trimmed
            .strip_prefix(prefix)
            .and_then(|s| s.strip_suffix("```"))
        {
            return body.trim();
        }
    }
    trimmed
}

pub fn validate(content: &str, sources: &[Source]) -> Result<Validated> {
    let draft: Draft =
        serde_json::from_str(json_content(content)).map_err(|error| DraftFormatError {
            detail: error.to_string(),
        })?;
    let mut claims = Vec::new();
    for input in draft.claims {
        let mut evidence = Vec::new();
        for item in input.evidence {
            let source = sources
                .iter()
                .find(|source| source.id == item.source_id)
                .context("Unavailable evidence source")?;
            let quote = match (item.passage_id, item.quote) {
                (Some(id), None) => passages(source)
                    .get(id)
                    .context("Unavailable passage ID")?
                    .quote
                    .clone(),
                (None, Some(quote)) => quote,
                _ => anyhow::bail!("Choose exactly one passage_id or legacy quote"),
            };
            evidence.push(Evidence {
                source_id: item.source_id,
                quote,
            });
        }
        claims.push(Claim {
            text: input.text,
            evidence,
        });
    }
    evidence::validate(claims, draft.questions, sources)
}
pub fn validate_review(content: &str, claim_count: usize) -> Result<()> {
    let review: Review =
        serde_json::from_str(json_content(content)).map_err(|error| ReviewFormatError {
            detail: error.to_string(),
        })?;
    evidence::validate_review(review, claim_count)
}
// Log only schema shape, never provider prose, quotes, questions or field names.
pub fn review_shape(content: &str) -> serde_json::Value {
    use serde_json::{Value, json};
    fn kind(value: &Value) -> &'static str {
        match value {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        }
    }
    match serde_json::from_str::<Value>(content) {
        Err(error) => {
            json!({"parse_error":format!("{:?}",error.classify()),"line":error.line(),"column":error.column(),"bytes":content.len()})
        }
        Ok(value) => json!({
            "root":kind(&value),
            "extra_root_fields":value.as_object().map(|o|o.keys().filter(|k| !["claims","questions_safe"].contains(&k.as_str())).count()),
            "questions_safe":kind(&value["questions_safe"]),
            "claims":kind(&value["claims"]),
            "verdicts":value["claims"].as_array().map(|claims|claims.iter().take(20).map(|v|json!({
                "index":v["index"].as_u64(),"index_type":kind(&v["index"]),"supported":kind(&v["supported"]),"reason":kind(&v["reason"]),
                "extra_fields":v.as_object().map(|o|o.keys().filter(|k| !["index","supported","reason"].contains(&k.as_str())).count())
            })).collect::<Vec<_>>())
        }),
    }
}

pub fn draft_shape(content: &str) -> serde_json::Value {
    use serde_json::{Value, json};
    fn kind(value: &Value) -> &'static str {
        match value {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        }
    }
    match serde_json::from_str::<Value>(json_content(content)) {
        Err(error) => {
            json!({"parse_error":format!("{:?}",error.classify()),"line":error.line(),"column":error.column(),"bytes":content.len()})
        }
        Ok(value) => json!({
            "root":kind(&value),
            "extra_root_fields":value.as_object().map(|o|o.keys().filter(|k| !["claims","questions"].contains(&k.as_str())).count()),
            "questions":kind(&value["questions"]),
            "claims":kind(&value["claims"]),
            "items":value["claims"].as_array().map(|claims|claims.iter().take(20).map(|claim|json!({
                "text":kind(&claim["text"]),"evidence":kind(&claim["evidence"]),
                "extra_fields":claim.as_object().map(|o|o.keys().filter(|k| !["text","evidence"].contains(&k.as_str())).count()),
                "citations":claim["evidence"].as_array().map(|items|items.iter().take(6).map(|v|json!({
                    "source_id":kind(&v["source_id"]),"passage_id":kind(&v["passage_id"]),"quote":kind(&v["quote"]),
                    "extra_fields":v.as_object().map(|o|o.keys().filter(|k| !["source_id","passage_id","quote"].contains(&k.as_str())).count())
                })).collect::<Vec<_>>())
            })).collect::<Vec<_>>())
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::evidence::{ReviewRejected, supported_part};
    use chrono::NaiveDate;
    use serde_json::json;
    fn source() -> Source {
        Source {
            id: 1,
            anchor: "paragraf-181".into(),
            kind: "paragraph".into(),
            citation: "Test source".into(),
            heading: String::new(),
            text: "Testovací text ustanovenia s podmienkou a výnimkou.".into(),
            context: "Úvodná podmienka. Testovací text ustanovenia s podmienkou a výnimkou.".into(),
            source_url: String::new(),
            pdf_url: String::new(),
            act_title: "Test".into(),
            valid_from: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            valid_to: None,
            verified_on: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            judgment: None,
            treaty: None,
            regulation: None,
            retrieval_methods: vec![],
        }
    }
    fn draft(id: i64, quote: &str) -> String {
        json!({"claims":[{"text":"Testovacie tvrdenie.","evidence":[{"source_id":id,"quote":quote}]}]}).to_string()
    }
    #[test]
    fn passage_ids_resolve_to_server_owned_text() {
        let data = json!({"claims":[{"text":"Testovacie tvrdenie.","evidence":[{"source_id":1,"passage_id":0}]}]}).to_string();
        let result = validate(&data, &[source()]).unwrap();
        assert_eq!(result.claims[0].evidence[0].quote, source().text);
        for invalid in [
            json!({"source_id":999,"passage_id":0}),
            json!({"source_id":1,"passage_id":999}),
            json!({"source_id":1,"passage_id":0,"quote":source().text}),
        ] {
            assert!(
                validate(
                    &json!({"claims":[{"text":"Test", "evidence":[invalid]}]}).to_string(),
                    &[source()]
                )
                .is_err()
            );
        }
    }
    #[test]
    fn outer_fences_preserve_evidence_and_do_not_accept_surrounding_prose() {
        let raw = draft(1, &source().text);
        for wrapped in [
            format!("```json\n{raw}\n```"),
            format!("```\r\n{raw}\r\n```\n"),
        ] {
            let result = validate(&wrapped, &[source()]).unwrap();
            assert_eq!(result.claims[0].evidence[0].quote, source().text);
        }
        assert!(validate(&format!("Here is the answer: {raw}"), &[source()]).is_err());
        assert!(
            validate(
                &format!("```json\n{}\n```", draft(999, &source().text)),
                &[source()]
            )
            .is_err()
        );
        assert!(
            validate(
                &format!("```json\n{}\n```", draft(1, "Invented quote")),
                &[source()]
            )
            .is_err()
        );
    }
    #[test]
    fn invalid_draft_schema_has_specific_feedback_and_safe_diagnostics() {
        for invalid in [
            json!({"source_id":1,"passage_id":[0]}),
            json!({"source_id":"Private value","passage_id":0}),
            json!({"source_id":1,"passage_ids":[0]}),
        ] {
            let raw =
                json!({"claims":[{"text":"Private claim", "evidence":[invalid]}],"questions":[]})
                    .to_string();
            let error = validate(&raw, &[source()]).err().unwrap();
            assert!(
                !error
                    .downcast_ref::<DraftFormatError>()
                    .unwrap()
                    .detail
                    .is_empty()
            );
            assert_eq!(error.to_string(), "Invalid claims format");
            assert!(!draft_shape(&raw).to_string().contains("Private"));
        }
        let raw = json!({"claims":[],"Private root":"Private value"}).to_string();
        assert_eq!(draft_shape(&raw)["extra_root_fields"], 1);
        assert!(!draft_shape(&raw).to_string().contains("Private"));
        assert_eq!(draft_shape("{")["parse_error"], "Eof");
    }
    #[test]
    fn long_passages_preserve_words_and_legal_text() {
        let mut data = source();
        data.text = (0..600)
            .map(|index| format!("slovenský_úryvok_{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        data.context = data.text.clone();
        let result = passages(&data);
        assert!(result.len() > 1);
        for (id, passage) in result.iter().enumerate() {
            assert_eq!(passage.id, id);
            assert!(data.text.contains(&passage.quote));
            assert!(passage.quote.chars().count() <= 2500);
        }
        for word in data.text.split_whitespace() {
            assert!(
                result
                    .iter()
                    .any(|p| p.quote.split_whitespace().any(|part| part == word))
            );
        }
    }
    #[test]
    fn review_feedback_does_not_leak_into_error_logs() {
        let error = validate_review(r#"{"claims":[{"index":0,"supported":false,"reason":"Private reasoning"}],"questions_safe":true}"#, 1).unwrap_err();
        assert!(!error.to_string().contains("Private reasoning"));
        assert_eq!(
            error.downcast_ref::<ReviewRejected>().unwrap().details[0].reason,
            "Private reasoning"
        );
    }
    #[test]
    fn review_shape_contains_no_private_content_or_unexpected_field_names() {
        let raw = json!({"claims":[{"index":0,"supported":"Private value","reason":"Private reasoning","Private key":"Private value"}],"questions_safe":true,"Private root":"Private value"}).to_string();
        let shape = review_shape(&raw);
        assert!(!shape.to_string().contains("Private"));
        assert_eq!(shape["extra_root_fields"], 1);
        assert_eq!(shape["verdicts"][0]["supported"], "string");
        assert_eq!(review_shape("{")["parse_error"], "Eof");
    }
    #[test]
    fn partial_answer_keeps_only_explicitly_supported_claims_and_their_sources() {
        let first = source();
        let mut second = source();
        second.id = 2;
        let draft = validate(
            &json!({"claims":[
            {"text":"Podložené tvrdenie.","evidence":[{"source_id":1,"passage_id":0}]},
            {"text":"Nepodložené tvrdenie.","evidence":[{"source_id":2,"passage_id":0}]}
        ],"questions":["Aký je dátum doručenia?"]})
            .to_string(),
            &[first, second],
        )
        .unwrap();
        let error = validate_review(r#"{"claims":[{"index":1,"supported":false},{"index":0,"supported":true}],"questions_safe":true}"#, 2).unwrap_err();
        let partial = supported_part(&draft, error.downcast_ref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(partial.claims.len(), 1);
        assert_eq!(partial.sources.len(), 1);
        assert_eq!(partial.sources[0].id, 1);
        assert_eq!(
            partial.answer,
            "Podložené tvrdenie. [N1]\n\nAký je dátum doručenia?"
        );
        let error = validate_review(r#"{"claims":[{"index":0,"supported":false},{"index":1,"supported":false}],"questions_safe":true}"#, 2).unwrap_err();
        assert!(
            supported_part(&draft, error.downcast_ref().unwrap())
                .unwrap()
                .is_none()
        );
        for review in [
            r#"{"claims":[{"index":0,"supported":true}],"questions_safe":true}"#,
            r#"{"claims":[{"index":0,"supported":true},{"index":0,"supported":false}],"questions_safe":true}"#,
        ] {
            let error = validate_review(review, 2).unwrap_err();
            assert!(error.downcast_ref::<ReviewRejected>().is_none());
        }
        let error = validate_review(r#"{"claims":[{"index":0,"supported":true},{"index":1,"supported":true}],"questions_safe":false}"#, 2).unwrap_err();
        let partial = supported_part(&draft, error.downcast_ref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(partial.claims.len(), 2);
        assert!(partial.questions.is_empty());
        assert!(!partial.answer.contains("Aký je dátum"));
    }
    #[test]
    fn real_quotes_are_attached_to_each_claim() {
        let result = validate(
            &draft(1, "Testovací text\n ustanovenia s podmienkou a výnimkou."),
            &[source()],
        )
        .unwrap();
        assert_eq!(result.answer, "Testovacie tvrdenie. [N1]");
        assert_eq!(result.sources.len(), 1);
        assert_eq!(result.claims.len(), 1);
    }
    #[test]
    fn invented_sources_quotes_and_unattributed_prose_are_rejected() {
        assert!(validate(&draft(999, &source().text), &[source()]).is_err());
        assert!(
            validate(
                &draft(1, "Tento vymyslený úryvok v zdroji nie je."),
                &[source()]
            )
            .is_err()
        );
        assert!(validate(&draft(1, "text"), &[source()]).is_err());
        assert!(
            validate(
                r#"{"answer":"Starý voľný formát", "source_ids":[1]}"#,
                &[source()]
            )
            .is_err()
        );
        assert!(
            validate(
                r#"{"claims":[{"text":"Bez dôkazu", "evidence":[]}]}"#,
                &[source()]
            )
            .is_err()
        );
        assert!(validate(r#"{"claims":[], "questions":[]}"#, &[]).is_err());
        assert!(validate("not JSON", &[]).is_err());
    }
    #[test]
    fn quote_must_belong_to_the_selected_source() {
        let mut other = source();
        other.id = 2;
        other.text = "Iný nesúvisiaci text ustanovenia.".into();
        other.context = other.text.clone();
        assert!(validate(&draft(2, &source().text), &[source(), other]).is_err());
    }
    #[test]
    fn clarification_does_not_require_a_fake_citation() {
        let result = validate(
            r#"{"claims":[], "questions":["Aký druh rozhodnutia vám bol doručený?"]}"#,
            &[],
        )
        .unwrap();
        assert!(result.sources.is_empty());
        assert!(result.claims.is_empty());
    }
    #[test]
    fn semantic_review_must_cover_every_claim_once() {
        assert!(
            validate_review(
                r#"{"claims":[{"index":0,"supported":true}],"questions_safe":true}"#,
                1
            )
            .is_ok()
        );
        for review in [
            r#"{"claims":[],"questions_safe":true}"#,
            r#"{"claims":[{"index":0,"supported":false}],"questions_safe":true}"#,
            r#"{"claims":[{"index":1,"supported":true}],"questions_safe":true}"#,
            r#"{"claims":[{"index":0,"supported":true}],"questions_safe":false}"#,
        ] {
            assert!(validate_review(review, 1).is_err());
        }
        assert!(validate_review(r#"{"claims":[{"index":0,"supported":true},{"index":0,"supported":true}],"questions_safe":true}"#, 2).is_err());
    }
}
