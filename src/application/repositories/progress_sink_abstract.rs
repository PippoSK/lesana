use crate::application::utils::events::ProgressEvent;
use std::{future::Future, pin::Pin};

pub trait ProgressSink: Send + Sync {
    fn emit(&self, event: ProgressEvent) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}
