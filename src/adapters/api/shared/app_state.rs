use super::chat_runs::ChatRunRegistry;
use crate::application::usecases::services::Services;
use std::{sync::Arc, time::Instant};
use tokio::sync::{Mutex, Semaphore};
#[derive(Clone)]
pub struct AppState {
    pub services: Arc<Services>,
    pub model_name: Option<String>,
    pub password: Arc<String>,
    pub secure_cookie: bool,
    pub chat_slots: Arc<Semaphore>,
    pub chat_runs: ChatRunRegistry,
    pub login_attempts: Arc<Mutex<Vec<Instant>>>,
}
