use crate::domain::Source;
use serde_json::{Value, json};
// Retrieval provenance is UI metadata, not evidence for a legal conclusion.
pub(crate) fn model_source(source: &Source) -> Value {
    let mut value = json!(source);
    value.as_object_mut().unwrap().remove("retrieval_methods");
    value
}

pub(crate) fn model_sources(sources: &[Source]) -> Vec<Value> {
    sources.iter().map(model_source).collect()
}
