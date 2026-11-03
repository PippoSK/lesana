use super::{ensure_owned_usecase::EnsureOwnedUseCase, interfaces::AbstractUseCase};
use crate::application::{
    contracts::{AttachmentDownload, AttachmentMetadata, PreparedAttachment},
    repositories::{AttachmentExtractor, AttachmentRepository, ConversationRepository},
    utils::errors::ApplicationError,
};
use anyhow::Result;
use uuid::Uuid;

pub const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_ATTACHMENTS: i64 = 5;
pub const MAX_ATTACHMENT_TEXT_CHARS: usize = 100_000;
pub const DOCX_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

pub struct AttachmentsUseCase<'a> {
    pub owner: Uuid,
    pub conversation: Uuid,
    pub conversations: &'a dyn ConversationRepository,
    pub attachments: &'a dyn AttachmentRepository,
    pub extractor: &'a dyn AttachmentExtractor,
}

impl AttachmentsUseCase<'_> {
    async fn authorize(&self) -> Result<()> {
        EnsureOwnedUseCase::new(self.owner, self.conversation, self.conversations)
            .execute()
            .await
    }

    pub async fn list(&self) -> Result<Vec<AttachmentMetadata>> {
        self.authorize().await?;
        self.attachments.list(self.owner, self.conversation).await
    }

    pub async fn upload(&self, filename: &str, bytes: Vec<u8>) -> Result<AttachmentMetadata> {
        self.authorize().await?;
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(ApplicationError::AttachmentTooLarge.into());
        }
        if bytes.is_empty() {
            return Err(ApplicationError::InvalidInput("Vyberte neprázdny súbor.").into());
        }
        let filename = sanitize_filename(filename)?;
        let content_type = content_type(&filename)?;
        let text = self.extractor.extract(content_type, &bytes).await?;
        let text = text.trim_start_matches('\u{feff}').trim().to_owned();
        if text.is_empty() {
            return Err(ApplicationError::InvalidInput(
                "Súbor neobsahuje čitateľný text. Pri skenovanom PDF najprv použite OCR.",
            )
            .into());
        }
        let text_chars = text.chars().count();
        if text_chars > MAX_ATTACHMENT_TEXT_CHARS {
            return Err(ApplicationError::InvalidInput(
                "Súbory v rozhovore môžu obsahovať spolu najviac 100 000 znakov textu.",
            )
            .into());
        }
        if text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\u{000c}'))
        {
            return Err(ApplicationError::InvalidInput(
                "Súbor obsahuje nepodporované riadiace znaky alebo binárne údaje.",
            )
            .into());
        }
        let metadata = AttachmentMetadata {
            id: Uuid::new_v4(),
            filename,
            content_type: content_type.to_owned(),
            size_bytes: bytes.len() as i64,
            text_chars: text_chars as i64,
        };
        self.attachments
            .insert(
                self.owner,
                self.conversation,
                PreparedAttachment {
                    metadata,
                    bytes,
                    text,
                },
            )
            .await
    }

    pub async fn download(&self, attachment: Uuid) -> Result<AttachmentDownload> {
        self.authorize().await?;
        self.attachments
            .download(self.owner, self.conversation, attachment)
            .await?
            .ok_or_else(|| ApplicationError::AttachmentNotFound.into())
    }

    pub async fn delete(&self, attachment: Uuid) -> Result<()> {
        self.authorize().await?;
        if !self
            .attachments
            .delete(self.owner, self.conversation, attachment)
            .await?
        {
            return Err(ApplicationError::AttachmentNotFound.into());
        }
        Ok(())
    }
}

fn sanitize_filename(filename: &str) -> Result<String> {
    let basename = filename.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let name: String = basename
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect();
    if name.is_empty() || name.len() > 240 || matches!(name.as_str(), "." | "..") {
        return Err(ApplicationError::InvalidInput(
            "Názov súboru je prázdny alebo príliš dlhý (najviac 240 bajtov).",
        )
        .into());
    }
    Ok(name)
}

fn content_type(filename: &str) -> Result<&'static str> {
    match filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "txt" => Ok("text/plain"),
        "md" | "markdown" => Ok("text/markdown"),
        "pdf" => Ok("application/pdf"),
        "docx" => Ok(DOCX_CONTENT_TYPE),
        _ => Err(ApplicationError::InvalidInput(
            "Podporované sú súbory PDF, TXT, MD a DOCX.",
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{
        contracts::{ConversationSummary, StoredMessage},
        repositories::PortFuture,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Conversations {
        exists: bool,
        owned: bool,
    }

    impl ConversationRepository for Conversations {
        fn session_exists(&self, _: Uuid) -> PortFuture<'_, bool> {
            Box::pin(async { Ok(self.exists) })
        }
        fn owns(&self, _: Uuid, _: Uuid) -> PortFuture<'_, bool> {
            Box::pin(async { Ok(self.owned) })
        }
        fn create_session(&self, _: Uuid) -> PortFuture<'_, ()> {
            unimplemented!()
        }
        fn list(&self, _: Uuid) -> PortFuture<'_, Vec<ConversationSummary>> {
            unimplemented!()
        }
        fn create(&self, _: Uuid, _: Uuid) -> PortFuture<'_, ()> {
            unimplemented!()
        }
        fn messages(&self, _: Uuid, _: Uuid) -> PortFuture<'_, Vec<StoredMessage>> {
            unimplemented!()
        }
        fn delete(&self, _: Uuid, _: Uuid) -> PortFuture<'_, bool> {
            unimplemented!()
        }
    }

    struct Files {
        extractions: AtomicUsize,
        writes: AtomicUsize,
    }

    impl AttachmentExtractor for Files {
        fn extract<'a>(&'a self, _: &'a str, bytes: &'a [u8]) -> PortFuture<'a, String> {
            Box::pin(async move {
                self.extractions.fetch_add(1, Ordering::SeqCst);
                Ok(std::str::from_utf8(bytes)?.to_owned())
            })
        }
    }

    impl AttachmentRepository for Files {
        fn insert(
            &self,
            _: Uuid,
            _: Uuid,
            file: PreparedAttachment,
        ) -> PortFuture<'_, AttachmentMetadata> {
            Box::pin(async move {
                self.writes.fetch_add(1, Ordering::SeqCst);
                Ok(file.metadata)
            })
        }
        fn list(&self, _: Uuid, _: Uuid) -> PortFuture<'_, Vec<AttachmentMetadata>> {
            unimplemented!()
        }
        fn download(
            &self,
            _: Uuid,
            _: Uuid,
            _: Uuid,
        ) -> PortFuture<'_, Option<AttachmentDownload>> {
            unimplemented!()
        }
        fn delete(&self, _: Uuid, _: Uuid, _: Uuid) -> PortFuture<'_, bool> {
            unimplemented!()
        }
    }

    #[test]
    fn filenames_remove_paths_and_header_controls_and_preserve_unicode() {
        assert_eq!(
            sanitize_filename("C:\\fakepath\\Zmluva Ž.pdf").unwrap(),
            "Zmluva Ž.pdf"
        );
        assert_eq!(
            sanitize_filename("../../zmluva\r\n.txt").unwrap(),
            "zmluva.txt"
        );
        assert!(sanitize_filename("..").is_err());
        assert!(sanitize_filename(&"a".repeat(241)).is_err());
        assert_eq!(content_type("ZMLUVA.PDF").unwrap(), "application/pdf");
        assert!(content_type("zmluva.exe").is_err());
    }

    #[tokio::test]
    async fn unauthorized_uploads_never_reach_extraction_or_storage() {
        let files = Files {
            extractions: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
        };
        for conversations in [
            Conversations {
                exists: false,
                owned: true,
            },
            Conversations {
                exists: true,
                owned: false,
            },
        ] {
            let usecase = AttachmentsUseCase {
                owner: Uuid::new_v4(),
                conversation: Uuid::new_v4(),
                conversations: &conversations,
                attachments: &files,
                extractor: &files,
            };
            assert!(usecase.upload("test.txt", b"private".to_vec()).await.is_err());
        }
        assert_eq!(files.extractions.load(Ordering::SeqCst), 0);
        assert_eq!(files.writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn invalid_files_are_never_saved_and_unicode_counts_as_characters() {
        let conversations = Conversations {
            exists: true,
            owned: true,
        };
        let files = Files {
            extractions: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
        };
        let usecase = AttachmentsUseCase {
            owner: Uuid::new_v4(),
            conversation: Uuid::new_v4(),
            conversations: &conversations,
            attachments: &files,
            extractor: &files,
        };
        for (filename, bytes) in [
            ("test.txt", Vec::new()),
            ("test.txt", vec![b'x'; MAX_ATTACHMENT_BYTES + 1]),
            ("test.exe", b"content".to_vec()),
            ("test.txt", b" \n\t ".to_vec()),
            ("test.txt", b"binary\0text".to_vec()),
            ("test.txt", vec![b'x'; MAX_ATTACHMENT_TEXT_CHARS + 1]),
        ] {
            assert!(usecase.upload(filename, bytes).await.is_err());
        }
        assert_eq!(files.writes.load(Ordering::SeqCst), 0);
        let file = usecase
            .upload("Príloha.txt", "ž".repeat(MAX_ATTACHMENT_TEXT_CHARS).into_bytes())
            .await
            .unwrap();
        assert_eq!(file.text_chars, MAX_ATTACHMENT_TEXT_CHARS as i64);
        assert_eq!(file.size_bytes, 2 * MAX_ATTACHMENT_TEXT_CHARS as i64);
        assert_eq!(files.writes.load(Ordering::SeqCst), 1);
    }
}
