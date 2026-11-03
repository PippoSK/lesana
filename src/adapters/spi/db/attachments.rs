use super::{PgStore, db_mappers::storage_error};
use crate::application::{
    contracts::{AttachmentDownload, AttachmentMetadata, PreparedAttachment},
    repositories::{AttachmentRepository, PortFuture},
    usecases::attachments_usecase::{MAX_ATTACHMENTS, MAX_ATTACHMENT_TEXT_CHARS},
    utils::errors::ApplicationError,
};
use anyhow::Result;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

async fn lock_conversation(
    db: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    conversation: Uuid,
) -> Result<()> {
    let locked: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM conversations WHERE id=$1 AND session_id=$2 FOR UPDATE NOWAIT",
    )
    .bind(conversation)
    .bind(owner)
    .fetch_optional(&mut **db)
    .await
    .map_err(|error| {
        if error.as_database_error().and_then(|e| e.code()).as_deref() == Some("55P03") {
            anyhow::Error::from(ApplicationError::ConversationBusy)
        } else {
            anyhow::Error::from(error)
        }
    })?;
    if locked.is_none() {
        return Err(ApplicationError::ConversationNotFound.into());
    }
    Ok(())
}

impl AttachmentRepository for PgStore {
    fn list(&self, owner: Uuid, conversation: Uuid) -> PortFuture<'_, Vec<AttachmentMetadata>> {
        Box::pin(async move {
            let result: Result<_> = async {
                Ok(sqlx::query_scalar::<_, sqlx::types::Json<AttachmentMetadata>>("SELECT jsonb_build_object('id',a.id,'filename',filename,'content_type',content_type,'size_bytes',size_bytes,'text_chars',text_chars) FROM conversation_attachments a JOIN conversations c ON c.id=a.conversation_id WHERE c.id=$1 AND c.session_id=$2 ORDER BY a.created_at,a.id")
                    .bind(conversation)
                    .bind(owner)
                    .fetch_all(&self.pool)
                    .await?
                    .into_iter()
                    .map(|r| r.0)
                    .collect())
            }
            .await;
            result.map_err(storage_error)
        })
    }

    fn insert(
        &self,
        owner: Uuid,
        conversation: Uuid,
        file: PreparedAttachment,
    ) -> PortFuture<'_, AttachmentMetadata> {
        Box::pin(async move {
            let result: Result<_> = async {
                let mut db = self.pool.begin().await?;
                lock_conversation(&mut db, owner, conversation).await?;
                let (count, chars): (i64, i64) = sqlx::query_as(
                    "SELECT count(*),coalesce(sum(text_chars),0)::bigint FROM conversation_attachments WHERE conversation_id=$1",
                )
                .bind(conversation)
                .fetch_one(&mut *db)
                .await?;
                if count >= MAX_ATTACHMENTS {
                    return Err(ApplicationError::InvalidInput(
                        "V rozhovore môže byť najviac 5 súborov. Pred pridaním ďalšieho niektorý odstráňte.",
                    )
                    .into());
                }
                if chars + file.metadata.text_chars > MAX_ATTACHMENT_TEXT_CHARS as i64 {
                    return Err(ApplicationError::InvalidInput(
                        "Súbory v rozhovore môžu obsahovať spolu najviac 100 000 znakov textu.",
                    )
                    .into());
                }
                sqlx::query("INSERT INTO conversation_attachments(id,conversation_id,filename,content_type,size_bytes,text_chars,original_bytes,extracted_text) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
                    .bind(file.metadata.id)
                    .bind(conversation)
                    .bind(&file.metadata.filename)
                    .bind(&file.metadata.content_type)
                    .bind(file.metadata.size_bytes)
                    .bind(file.metadata.text_chars)
                    .bind(file.bytes)
                    .bind(file.text)
                    .execute(&mut *db)
                    .await?;
                sqlx::query("UPDATE conversations SET updated_at=now() WHERE id=$1")
                    .bind(conversation)
                    .execute(&mut *db)
                    .await?;
                db.commit().await?;
                Ok(file.metadata)
            }
            .await;
            result.map_err(storage_error)
        })
    }

    fn download(
        &self,
        owner: Uuid,
        conversation: Uuid,
        attachment: Uuid,
    ) -> PortFuture<'_, Option<AttachmentDownload>> {
        Box::pin(async move {
            let result: Result<_> = async {
                let row: Option<(sqlx::types::Json<AttachmentMetadata>, Vec<u8>)> = sqlx::query_as("SELECT jsonb_build_object('id',a.id,'filename',filename,'content_type',content_type,'size_bytes',size_bytes,'text_chars',text_chars),original_bytes FROM conversation_attachments a JOIN conversations c ON c.id=a.conversation_id WHERE c.id=$1 AND c.session_id=$2 AND a.id=$3")
                    .bind(conversation)
                    .bind(owner)
                    .bind(attachment)
                    .fetch_optional(&self.pool)
                    .await?;
                Ok(row.map(|(metadata, bytes)| AttachmentDownload {
                    metadata: metadata.0,
                    bytes,
                }))
            }
            .await;
            result.map_err(storage_error)
        })
    }

    fn delete(
        &self,
        owner: Uuid,
        conversation: Uuid,
        attachment: Uuid,
    ) -> PortFuture<'_, bool> {
        Box::pin(async move {
            let result: Result<_> = async {
                let mut db = self.pool.begin().await?;
                lock_conversation(&mut db, owner, conversation).await?;
                let removed = sqlx::query(
                    "DELETE FROM conversation_attachments WHERE id=$1 AND conversation_id=$2",
                )
                .bind(attachment)
                .bind(conversation)
                .execute(&mut *db)
                .await?
                .rows_affected()
                    != 0;
                if removed {
                    sqlx::query("UPDATE conversations SET updated_at=now() WHERE id=$1")
                        .bind(conversation)
                        .execute(&mut *db)
                        .await?;
                }
                db.commit().await?;
                Ok(removed)
            }
            .await;
            result.map_err(storage_error)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::repositories::{ConversationRepository, TurnRepository};
    use std::time::Duration;

    fn file(text: &str) -> PreparedAttachment {
        PreparedAttachment {
            metadata: AttachmentMetadata {
                id: Uuid::new_v4(),
                filename: "zmluva.txt".into(),
                content_type: "text/plain".into(),
                size_bytes: text.len() as i64,
                text_chars: text.chars().count() as i64,
            },
            bytes: text.as_bytes().to_vec(),
            text: text.into(),
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    #[ignore = "requires disposable PostgreSQL via DATABASE_URL"]
    async fn attachments_enforce_ownership_limits_turn_lock_and_cascade(pool: sqlx::PgPool) {
        let store = PgStore {
            pool,
            embeddings: None,
        };
        let owner = Uuid::new_v4();
        let stranger = Uuid::new_v4();
        let conversation = Uuid::new_v4();
        store.create_session(owner).await.unwrap();
        store.create_session(stranger).await.unwrap();
        store.create(owner, conversation).await.unwrap();
        let added = store
            .insert(owner, conversation, file("Príloha"))
            .await
            .unwrap();
        assert!(
            store
                .download(stranger, conversation, added.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            AttachmentRepository::list(&store, stranger, conversation)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .insert(stranger, conversation, file("Cudzí"))
                .await
                .is_err()
        );
        assert!(
            AttachmentRepository::delete(&store, stranger, conversation, added.id)
                .await
                .is_err()
        );
        assert_eq!(
            store
                .download(owner, conversation, added.id)
                .await
                .unwrap()
                .unwrap()
                .bytes,
            "Príloha".as_bytes()
        );
        let mut turn = store.begin_turn(owner, conversation).await.unwrap();
        assert_eq!(turn.attachments().await.unwrap().len(), 1);
        assert!(matches!(
            store
                .insert(owner, conversation, file("Ďalší"))
                .await
                .unwrap_err()
                .downcast_ref::<ApplicationError>(),
            Some(ApplicationError::ConversationBusy)
        ));
        assert!(
            AttachmentRepository::delete(&store, owner, conversation, added.id)
                .await
                .is_err()
        );
        drop(turn);
        // A dropped transaction rolls back asynchronously before releasing its lock.
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match store.insert(owner, conversation, file("Text")).await {
                    Ok(_) => break,
                    Err(error)
                        if matches!(
                            error.downcast_ref::<ApplicationError>(),
                            Some(ApplicationError::ConversationBusy)
                        ) =>
                    {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(error) => panic!("Attachment insert failed: {error}"),
                }
            }
        })
        .await
        .expect("dropped turn releases the lock");
        for _ in 0..3 {
            store.insert(owner, conversation, file("Text")).await.unwrap();
        }
        assert!(
            store
                .insert(owner, conversation, file("Šiesty"))
                .await
                .is_err()
        );
        assert!(
            AttachmentRepository::delete(&store, owner, conversation, added.id)
                .await
                .unwrap()
        );
        assert!(
            store
                .insert(
                    owner,
                    conversation,
                    file(&"a".repeat(MAX_ATTACHMENT_TEXT_CHARS)),
                )
                .await
                .is_err()
        );
        ConversationRepository::delete(&store, owner, conversation)
            .await
            .unwrap();
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM conversation_attachments")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0);
    }
}
