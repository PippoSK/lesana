//! Provider wire schemas. Domain validation still checks citations and legal support.
use crate::application::contracts::CompletionKind;
use serde_json::{Value, json};

/// Keep max reasoning while bounding the visible answer and review work.
/// Independent domain and evidence validation still apply to the result.
pub(super) fn deepseek_response_format(kind: CompletionKind) -> Value {
    let mut format = response_format(kind);
    let properties = &mut format["json_schema"]["schema"]["properties"];
    match kind {
        CompletionKind::Answer => {
            properties["claims"]["maxItems"] = json!(6);
            let claim = &mut properties["claims"]["items"]["properties"];
            claim["text"]["maxLength"] = json!(1800);
            claim["evidence"]["minItems"] = json!(1);
            claim["evidence"]["maxItems"] = json!(6);
            properties["questions"]["maxItems"] = json!(4);
            properties["questions"]["items"]["maxLength"] = json!(500);
        }
        CompletionKind::EvidenceReview => {
            // DeepSeek's review policy sends at most one claim per call.
            properties["claims"]["maxItems"] = json!(1);
            properties["claims"]["items"]["properties"]["reason"]["maxLength"] = json!(250);
        }
    }
    format
}

pub(super) fn response_format(kind: CompletionKind) -> Value {
    let (name, schema) = match kind {
        CompletionKind::Answer => (
            "lesana_answer",
            json!({
                "type": "object",
                "properties": {
                    "claims": {"type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "text": {"type": "string"},
                            "evidence": {"type": "array", "items": {
                                "type": "object",
                                "properties": {
                                    "source_id": {"type": "integer"},
                                    "passage_id": {"type": "integer"}
                                },
                                "required": ["source_id", "passage_id"],
                                "additionalProperties": false
                            }}
                        },
                        "required": ["text", "evidence"],
                        "additionalProperties": false
                    }},
                    "questions": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["claims", "questions"],
                "additionalProperties": false
            }),
        ),
        CompletionKind::EvidenceReview => (
            "lesana_evidence_review",
            json!({
                "type": "object",
                "properties": {
                    "claims": {"type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "index": {"type": "integer"},
                            "supported": {"type": "boolean"},
                            "reason": {"type": "string"}
                        },
                        "required": ["index", "supported", "reason"],
                        "additionalProperties": false
                    }},
                    "questions_safe": {"type": "boolean"}
                },
                "required": ["claims", "questions_safe"],
                "additionalProperties": false
            }),
        ),
    };
    json!({"type": "json_schema", "json_schema": {"name": name, "strict": true, "schema": schema}})
}
