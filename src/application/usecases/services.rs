//! Entry-adapter facade: assemble typed use cases from injected output ports.
pub use super::search_usecase::SearchResult;
use super::{
    agent::RunAnswer,
    chat_usecase::{ChatRequest, ChatService},
    check_health_usecase::CheckHealthUseCase,
    create_conversation_usecase::CreateConversationUseCase,
    delete_conversation_usecase::DeleteConversationUseCase,
    ensure_owned_usecase::EnsureOwnedUseCase,
    get_conversation_usecase::GetConversationUseCase,
    get_corpus_usecase::GetCorpusUseCase,
    get_source_usecase::GetSourceUseCase,
    interfaces::AbstractUseCase,
    list_conversations_usecase::ListConversationsUseCase,
    process_chat_usecase::ProcessChatUseCase,
    require_session_usecase::RequireSessionUseCase,
    retrieval::{Architecture, Config},
    search_usecase::SearchUseCase,
    start_session_usecase::StartSessionUseCase,
    validate_chat_usecase::ValidateChatUseCase,
};
use crate::{
    application::{contracts::*, repositories::*, utils::events::Events},
    domain::Source,
};
use anyhow::Result;
use chrono::NaiveDate;
use std::sync::Arc;
use uuid::Uuid;

pub struct Services {
    chat: ChatService,
    conversations: Arc<dyn ConversationRepository>,
    catalog: Arc<dyn CatalogRepository>,
    source_requests: Option<Arc<dyn SourceRequestRepository>>,
    attachments: Option<Arc<dyn AttachmentRepository>>,
    attachment_extractor: Option<Arc<dyn AttachmentExtractor>>,
}
impl Services {
    pub fn new(
        chat: ChatService,
        conversations: Arc<dyn ConversationRepository>,
        catalog: Arc<dyn CatalogRepository>,
    ) -> Self {
        Self {
            chat,
            conversations,
            catalog,
            source_requests: None,
            attachments: None,
            attachment_extractor: None,
        }
    }
    pub fn with_source_requests(mut self, repository: Arc<dyn SourceRequestRepository>) -> Self {
        self.source_requests = Some(repository);
        self
    }
    pub fn with_attachments(
        mut self,
        repository: Arc<dyn AttachmentRepository>,
        extractor: Arc<dyn AttachmentExtractor>,
    ) -> Self {
        self.attachments = Some(repository);
        self.attachment_extractor = Some(extractor);
        self
    }
    fn attachments_usecase(
        &self,
        owner: Uuid,
        conversation: Uuid,
    ) -> Result<super::attachments_usecase::AttachmentsUseCase<'_>> {
        Ok(super::attachments_usecase::AttachmentsUseCase {
            owner,
            conversation,
            conversations: self.conversations.as_ref(),
            attachments: self
                .attachments
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Attachments are not configured"))?,
            extractor: self
                .attachment_extractor
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Attachment extraction is not configured"))?,
        })
    }
    pub async fn list_attachments(
        &self,
        owner: Uuid,
        conversation: Uuid,
    ) -> Result<Vec<AttachmentMetadata>> {
        self.attachments_usecase(owner, conversation)?.list().await
    }
    pub async fn upload_attachment(
        &self,
        owner: Uuid,
        conversation: Uuid,
        filename: &str,
        bytes: Vec<u8>,
    ) -> Result<AttachmentMetadata> {
        self.attachments_usecase(owner, conversation)?
            .upload(filename, bytes)
            .await
    }
    pub async fn download_attachment(
        &self,
        owner: Uuid,
        conversation: Uuid,
        attachment: Uuid,
    ) -> Result<AttachmentDownload> {
        self.attachments_usecase(owner, conversation)?
            .download(attachment)
            .await
    }
    pub async fn delete_attachment(
        &self,
        owner: Uuid,
        conversation: Uuid,
        attachment: Uuid,
    ) -> Result<()> {
        self.attachments_usecase(owner, conversation)?
            .delete(attachment)
            .await
    }
    fn source_requests_usecase(
        &self,
        owner: Uuid,
    ) -> Result<super::source_requests_usecase::SourceRequestsUseCase<'_>> {
        Ok(super::source_requests_usecase::SourceRequestsUseCase {
            owner,
            sessions: self.conversations.as_ref(),
            requests: self
                .source_requests
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Source requests are not configured"))?,
        })
    }
    pub async fn list_source_requests(&self, owner: Uuid) -> Result<Vec<SourceRequest>> {
        self.source_requests_usecase(owner)?.list().await
    }
    pub async fn request_source(
        &self,
        owner: Uuid,
        kind: &str,
        reference: &str,
    ) -> Result<SourceRequest> {
        self.source_requests_usecase(owner)?
            .enqueue(kind, reference)
            .await
    }
    /// Called only after the entry adapter authenticates the login credentials.
    pub async fn start_session(&self) -> Result<Uuid> {
        StartSessionUseCase::new(self.conversations.as_ref())
            .execute()
            .await
    }
    pub async fn require_session(&self, owner: Uuid) -> Result<()> {
        RequireSessionUseCase::new(owner, self.conversations.as_ref())
            .execute()
            .await
    }
    pub async fn health(&self) -> Result<()> {
        CheckHealthUseCase::new(self.catalog.as_ref())
            .execute()
            .await
    }
    pub async fn corpus(&self) -> Result<CorpusSnapshot> {
        GetCorpusUseCase::new(self.catalog.as_ref()).execute().await
    }
    pub fn model_enabled(&self) -> bool {
        self.chat.model.is_some()
    }
    pub fn turn_timeout_seconds(&self) -> u64 {
        self.chat.turn_timeout_seconds()
    }
    pub fn rag_config(&self) -> &Config {
        &self.chat.config
    }
    pub async fn list_conversations(&self, owner: Uuid) -> Result<Vec<ConversationSummary>> {
        ListConversationsUseCase::new(owner, self.conversations.as_ref())
            .execute()
            .await
    }
    pub async fn create_conversation(&self, owner: Uuid) -> Result<CreatedConversation> {
        CreateConversationUseCase::new(owner, self.conversations.as_ref())
            .execute()
            .await
    }
    pub async fn ensure_owned(&self, owner: Uuid, id: Uuid) -> Result<()> {
        EnsureOwnedUseCase::new(owner, id, self.conversations.as_ref())
            .execute()
            .await
    }
    pub async fn get_conversation(&self, owner: Uuid, id: Uuid) -> Result<Vec<StoredMessage>> {
        GetConversationUseCase::new(owner, id, self.conversations.as_ref())
            .execute()
            .await
    }
    pub async fn delete_conversation(&self, owner: Uuid, id: Uuid) -> Result<()> {
        DeleteConversationUseCase::new(owner, id, self.conversations.as_ref())
            .execute()
            .await
    }
    pub async fn get_source(&self, id: i64) -> Result<Source> {
        GetSourceUseCase::new(id, self.chat.legal.as_ref())
            .execute()
            .await
    }
    pub async fn validate_chat(&self, owner: Uuid, id: Uuid, question: &str) -> Result<()> {
        ValidateChatUseCase::new(owner, id, question, self.conversations.as_ref(), &self.chat)
            .execute()
            .await
    }
    pub async fn process_chat(
        &self,
        owner: Uuid,
        id: Uuid,
        question: &str,
        date: NaiveDate,
        architecture: Architecture,
        events: &Events,
        deadline: tokio::time::Instant,
    ) -> Result<RunAnswer> {
        ProcessChatUseCase::new(
            ChatRequest {
                owner,
                conversation: id,
                question,
                date,
                architecture,
                events,
                deadline,
            },
            self.conversations.as_ref(),
            &self.chat,
        )
        .execute()
        .await
    }
    pub async fn search(&self, query: &str, date: NaiveDate) -> Result<SearchResult> {
        SearchUseCase::new(query, date, self.chat.legal.as_ref())
            .execute()
            .await
    }
}
