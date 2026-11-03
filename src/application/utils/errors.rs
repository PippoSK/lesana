#[derive(Debug)]
pub enum ModelFailure {
    Timeout,
    Connection,
    Http(u16),
    Quota,
    InvalidResponse,
    OutputLimit,
}
impl std::fmt::Display for ModelFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "Model request timed out"),
            Self::Connection => write!(f, "Model connection failed"),
            Self::Http(status) => write!(f, "Model returned HTTP {status}"),
            Self::Quota => write!(f, "Model account quota exhausted"),
            Self::InvalidResponse => write!(f, "Invalid model response"),
            Self::OutputLimit => write!(f, "Model output token limit reached"),
        }
    }
}
impl std::error::Error for ModelFailure {}

#[derive(Debug)]
pub enum ApplicationError {
    InvalidInput(&'static str),
    ConversationNotFound,
    SourceNotFound,
    AttachmentNotFound,
    AttachmentTooLarge,
    ConversationBusy,
    SessionExpired,
}
impl std::fmt::Display for ApplicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(message) => f.write_str(message),
            Self::ConversationNotFound => f.write_str("Rozhovor sa nenašiel."),
            Self::SourceNotFound => f.write_str("Ustanovenie sa nenašlo."),
            Self::AttachmentNotFound => f.write_str("Súbor sa nenašiel."),
            Self::AttachmentTooLarge => f.write_str("Súbor môže mať najviac 10 MiB."),
            Self::ConversationBusy => {
                f.write_str("Prebieha spracovanie rozhovoru. Skúste to o chvíľu.")
            }
            Self::SessionExpired => f.write_str("Relácia vypršala. Prihláste sa znova."),
        }
    }
}
impl std::error::Error for ApplicationError {}

/// Infrastructure failures are distinguished from invalid retrieval arguments.
#[derive(Debug)]
pub struct StorageFailure;
impl std::fmt::Display for StorageFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Storage operation failed")
    }
}
impl std::error::Error for StorageFailure {}
