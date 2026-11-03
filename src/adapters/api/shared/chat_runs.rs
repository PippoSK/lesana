//! Short-lived transport state. Completed answers remain in conversation storage.
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;

const RETENTION: Duration = Duration::from_secs(60 * 60);
const MAX_ENTRIES: usize = 1024;
type RunKey = (Uuid, Uuid);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ChatRunStatus {
    Running { text: String },
    Completed,
    Failed { error: String, code: String },
}

struct RunEntry {
    status: ChatRunStatus,
    finished_at: Option<Instant>,
}

#[derive(Clone, Default)]
pub struct ChatRunRegistry {
    entries: Arc<Mutex<HashMap<RunKey, RunEntry>>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum StartRunError {
    Duplicate,
    Full,
}

impl ChatRunRegistry {
    pub fn start(&self, conversation: Uuid, request: Uuid) -> Result<(), StartRunError> {
        self.start_at(conversation, request, Instant::now())
    }

    fn start_at(&self, conversation: Uuid, request: Uuid, now: Instant) -> Result<(), StartRunError> {
        let mut entries = self.entries.lock().unwrap_or_else(|error| error.into_inner());
        Self::prune(&mut entries, now);
        if entries.contains_key(&(conversation, request)) {
            return Err(StartRunError::Duplicate);
        }
        if entries.len() >= MAX_ENTRIES {
            let oldest = entries
                .iter()
                .filter_map(|(key, entry)| entry.finished_at.map(|finished| (*key, finished)))
                .min_by_key(|(_, finished)| *finished)
                .map(|(key, _)| key);
            if let Some(key) = oldest {
                entries.remove(&key);
            } else {
                return Err(StartRunError::Full);
            }
        }
        entries.insert(
            (conversation, request),
            RunEntry {
                status: ChatRunStatus::Running {
                    text: "Pripravujem právnu rešerš…".into(),
                },
                finished_at: None,
            },
        );
        Ok(())
    }

    pub fn get(&self, conversation: Uuid, request: Uuid) -> Option<ChatRunStatus> {
        self.get_at(conversation, request, Instant::now())
    }

    fn get_at(&self, conversation: Uuid, request: Uuid, now: Instant) -> Option<ChatRunStatus> {
        let mut entries = self.entries.lock().unwrap_or_else(|error| error.into_inner());
        Self::prune(&mut entries, now);
        entries
            .get(&(conversation, request))
            .map(|entry| entry.status.clone())
    }

    pub fn progress(&self, conversation: Uuid, request: Uuid, text: String) {
        self.update(conversation, request, ChatRunStatus::Running { text });
    }

    pub fn complete(&self, conversation: Uuid, request: Uuid) {
        self.update(conversation, request, ChatRunStatus::Completed);
    }

    pub fn fail(&self, conversation: Uuid, request: Uuid, error: &str, code: &str) {
        self.update(
            conversation,
            request,
            ChatRunStatus::Failed {
                error: error.into(),
                code: code.into(),
            },
        );
    }

    fn update(&self, conversation: Uuid, request: Uuid, status: ChatRunStatus) {
        let mut entries = self.entries.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(entry) = entries.get_mut(&(conversation, request)) {
            if entry.finished_at.is_none() {
                if !matches!(status, ChatRunStatus::Running { .. }) {
                    entry.finished_at = Some(Instant::now());
                }
                entry.status = status;
            }
        }
    }

    fn prune(entries: &mut HashMap<RunKey, RunEntry>, now: Instant) {
        entries.retain(|_, entry| {
            entry
                .finished_at
                .is_none_or(|finished| now.saturating_duration_since(finished) < RETENTION)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_isolated_by_conversation_and_duplicates_keep_the_original_run() {
        let runs = ChatRunRegistry::default();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let request = Uuid::new_v4();
        runs.start(first, request).unwrap();
        runs.progress(first, request, "Kontrolujem podklady…".into());
        assert_eq!(runs.start(first, request), Err(StartRunError::Duplicate));
        assert_eq!(runs.get(second, request), None);
        runs.start(second, request).unwrap();
        runs.complete(first, request);
        runs.progress(first, request, "Stará udalosť".into());
        assert_eq!(runs.get(first, request), Some(ChatRunStatus::Completed));
        assert!(matches!(
            runs.get(second, request),
            Some(ChatRunStatus::Running { .. })
        ));
        runs.fail(second, request, "Model je nedostupný.", "model_unavailable");
        assert_eq!(
            runs.get(second, request),
            Some(ChatRunStatus::Failed {
                error: "Model je nedostupný.".into(),
                code: "model_unavailable".into(),
            })
        );
        assert_eq!(runs.start(first, request), Err(StartRunError::Duplicate));
    }

    #[test]
    fn concurrent_registration_accepts_a_request_only_once() {
        let runs = ChatRunRegistry::default();
        let conversation = Uuid::new_v4();
        let request = Uuid::new_v4();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let tasks: Vec<_> = (0..2)
            .map(|_| {
                let runs = runs.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    runs.start(conversation, request)
                })
            })
            .collect();
        let results: Vec<_> = tasks.into_iter().map(|task| task.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == Err(StartRunError::Duplicate))
                .count(),
            1
        );
    }

    #[test]
    fn only_finished_runs_expire_after_an_hour() {
        let runs = ChatRunRegistry::default();
        let conversation = Uuid::new_v4();
        let finished = Uuid::new_v4();
        let active = Uuid::new_v4();
        runs.start(conversation, finished).unwrap();
        runs.start(conversation, active).unwrap();
        runs.complete(conversation, finished);
        let later = Instant::now() + RETENTION;
        assert_eq!(runs.get_at(conversation, finished, later), None);
        assert!(matches!(
            runs.get_at(conversation, active, later),
            Some(ChatRunStatus::Running { .. })
        ));
    }

    #[test]
    fn capacity_evicts_finished_runs_and_never_active_runs() {
        let runs = ChatRunRegistry::default();
        let conversation = Uuid::new_v4();
        let first = Uuid::new_v4();
        runs.start(conversation, first).unwrap();
        for _ in 1..MAX_ENTRIES {
            runs.start(conversation, Uuid::new_v4()).unwrap();
        }
        assert_eq!(
            runs.start(conversation, Uuid::new_v4()),
            Err(StartRunError::Full)
        );
        runs.complete(conversation, first);
        let replacement = Uuid::new_v4();
        runs.start(conversation, replacement).unwrap();
        assert_eq!(runs.get(conversation, first), None);
        assert_eq!(runs.entries.lock().unwrap().len(), MAX_ENTRIES);
        assert!(matches!(
            runs.get(conversation, replacement),
            Some(ChatRunStatus::Running { .. })
        ));
    }

    #[test]
    fn response_shape_does_not_include_question_or_answer_data() {
        assert_eq!(
            serde_json::to_value(ChatRunStatus::Completed).unwrap(),
            serde_json::json!({"state":"completed"})
        );
        assert_eq!(
            serde_json::to_value(ChatRunStatus::Running {
                text: "Pracujem…".into(),
            })
            .unwrap(),
            serde_json::json!({"state":"running","text":"Pracujem…"})
        );
        assert_eq!(
            serde_json::to_value(ChatRunStatus::Failed {
                error: "Chyba".into(),
                code: "timeout".into(),
            })
            .unwrap(),
            serde_json::json!({"state":"failed","error":"Chyba","code":"timeout"})
        );
    }
}
