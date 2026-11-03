use crate::adapters::api::shared::current_date::today;
use crate::{adapters::api::shared::app_state::AppState, application::contracts::CorpusSnapshot};
use serde_json::{Value, json};
pub fn corpus(app: &AppState, snapshot: CorpusSnapshot) -> Value {
    json!({"versions":snapshot.versions,"agent_enabled":app.services.model_enabled(),"model":app.model_name,"embeddings":snapshot.embeddings,"default_date":today(),"password_required":!app.password.is_empty(),"rag":{"engine":"rig","default_architecture":app.services.rag_config().default,"architectures":["pipeline","agentic"],"max_turns":app.services.rag_config().max_turns}})
}
