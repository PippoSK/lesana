use super::{db_mappers::storage_error, embedding_index::Embeddings};
use crate::application::{
    contracts::*, repositories::*, usecases::agent::RunAnswer, utils::errors::ApplicationError,
};
use anyhow::Result;
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(Clone)]
pub struct PgStore {
    pub pool: PgPool,
    pub embeddings: Option<Embeddings>,
}
impl ConversationRepository for PgStore {
    fn session_exists(&self, id: Uuid) -> PortFuture<'_, bool> {
        Box::pin(async move {
            let result: Result<_> = async {
                Ok(sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=$1 AND expires_at>now())",
                )
                .bind(id)
                .fetch_one(&self.pool)
                .await?)
            }
            .await;
            result.map_err(storage_error)
        })
    }
    fn create_session(&self, id: Uuid) -> PortFuture<'_, ()> {
        Box::pin(async move {
            let result: Result<_> = async {
                sqlx::query("INSERT INTO sessions(id) VALUES($1)")
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            }
            .await;
            result.map_err(storage_error)
        })
    }
    fn list(&self, owner: Uuid) -> PortFuture<'_, Vec<ConversationSummary>> {
        Box::pin(async move {
            let result: Result<_> = async {
        Ok(sqlx::query_scalar::<_, sqlx::types::Json<ConversationSummary>>("SELECT jsonb_build_object('id',id,'title',title,'updated_at',updated_at) FROM conversations WHERE session_id=$1 ORDER BY updated_at DESC LIMIT 100").bind(owner).fetch_all(&self.pool).await?.into_iter().map(|row| row.0).collect())
        }.await;
            result.map_err(storage_error)
        })
    }
    fn create(&self, owner: Uuid, id: Uuid) -> PortFuture<'_, ()> {
        Box::pin(async move {
            let result: Result<_> = async {
                sqlx::query("INSERT INTO conversations(id,session_id) VALUES($1,$2)")
                    .bind(id)
                    .bind(owner)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            }
            .await;
            result.map_err(storage_error)
        })
    }
    fn owns(&self, owner: Uuid, id: Uuid) -> PortFuture<'_, bool> {
        Box::pin(async move {
            let result: Result<_> = async {
                Ok(sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM conversations WHERE id=$1 AND session_id=$2)",
                )
                .bind(id)
                .bind(owner)
                .fetch_one(&self.pool)
                .await?)
            }
            .await;
            result.map_err(storage_error)
        })
    }
    fn messages(&self, owner: Uuid, id: Uuid) -> PortFuture<'_, Vec<StoredMessage>> {
        Box::pin(async move {
            let result: Result<_> = async {
        Ok(sqlx::query_scalar::<_, sqlx::types::Json<StoredMessage>>("SELECT jsonb_build_object('id',m.id,'role',role,'content',content,'sources',sources,'claims',claims,'as_of',as_of,'mode',mode,'architecture',architecture,'tool_calls',tool_calls) FROM messages m JOIN conversations c ON c.id=m.conversation_id WHERE m.conversation_id=$1 AND c.session_id=$2 ORDER BY m.id").bind(id).bind(owner).fetch_all(&self.pool).await?.into_iter().map(|row| row.0).collect())
        }.await;
            result.map_err(storage_error)
        })
    }
    fn delete(&self, owner: Uuid, id: Uuid) -> PortFuture<'_, bool> {
        Box::pin(async move {
            let result: Result<_> = async {
                let result = sqlx::query("DELETE FROM conversations WHERE id=$1 AND session_id=$2")
                    .bind(id)
                    .bind(owner)
                    .execute(&self.pool)
                    .await?;
                Ok(result.rows_affected() != 0)
            }
            .await;
            result.map_err(storage_error)
        })
    }
}
struct PgTurn {
    db: Transaction<'static, Postgres>,
    conversation: Uuid,
}
impl TurnRepository for PgStore {
    fn begin_turn(
        &self,
        owner: Uuid,
        conversation: Uuid,
    ) -> PortFuture<'_, Box<dyn ConversationTurn>> {
        Box::pin(async move {
            let result: Result<_> = async {
                let mut db = self.pool.begin().await?;
                let locked: Option<Uuid> = sqlx::query_scalar(
                    "SELECT id FROM conversations WHERE id=$1 AND session_id=$2 FOR UPDATE NOWAIT",
                )
                .bind(conversation)
                .bind(owner)
                .fetch_optional(&mut *db)
                .await?;
                if locked.is_none() {
                    return Err(ApplicationError::ConversationNotFound.into());
                }
                Ok(Box::new(PgTurn { db, conversation }) as Box<dyn ConversationTurn>)
            }
            .await;
            result.map_err(storage_error)
        })
    }
}
impl ConversationTurn for PgTurn {
    fn attachments(&mut self) -> PortFuture<'_, Vec<AttachmentContext>> {
        Box::pin(async move {
            let result: Result<_> = async {
                Ok(sqlx::query_scalar::<_, sqlx::types::Json<AttachmentContext>>(
                    "SELECT jsonb_build_object('id',id,'filename',filename,'text',extracted_text) FROM conversation_attachments WHERE conversation_id=$1 ORDER BY created_at,id",
                )
                .bind(self.conversation)
                .fetch_all(&mut *self.db)
                .await?
                .into_iter()
                .map(|row| row.0)
                .collect())
            }
            .await;
            result.map_err(storage_error)
        })
    }
    fn history(&mut self) -> PortFuture<'_, Vec<ModelMessage>> {
        Box::pin(async move {
            let result: Result<_> = async {
        Ok(sqlx::query_scalar::<_, sqlx::types::Json<ModelMessage>>("SELECT jsonb_build_object('role',role,'content',content) FROM (SELECT id,role,content FROM messages WHERE conversation_id=$1 ORDER BY id DESC LIMIT 16) history ORDER BY id")
            .bind(self.conversation).fetch_all(&mut *self.db).await?.into_iter().map(|row| row.0).collect())
        }.await;
            result.map_err(storage_error)
        })
    }
    fn commit<'a>(
        mut self: Box<Self>,
        question: &'a str,
        run: &'a RunAnswer,
    ) -> PortFuture<'a, ()> {
        Box::pin(async move {
            let result: Result<_> = async {
        let answer=&run.content;
        sqlx::query("INSERT INTO messages(conversation_id,role,content,as_of,mode,architecture) VALUES($1,'user',$2,$3,$4,$5)")
            .bind(self.conversation).bind(question).bind(answer.as_of).bind(&answer.mode).bind(run.architecture.as_str()).execute(&mut *self.db).await?;
        sqlx::query("INSERT INTO messages(conversation_id,role,content,sources,as_of,mode,claims,architecture,tool_calls) VALUES($1,'assistant',$2,$3,$4,$5,$6,$7,$8)")
            .bind(self.conversation).bind(&answer.answer).bind(json!(&answer.sources)).bind(answer.as_of).bind(&answer.mode).bind(json!(&answer.claims)).bind(run.architecture.as_str()).bind(json!(&run.tool_calls)).execute(&mut *self.db).await?;
        sqlx::query("UPDATE conversations SET updated_at=now(),title=CASE WHEN title='Nový rozhovor' THEN $2 ELSE title END WHERE id=$1")
            .bind(self.conversation).bind(question.chars().take(65).collect::<String>()).execute(&mut *self.db).await?;
        self.db.commit().await?;
        Ok(())
        }.await;
            result.map_err(storage_error)
        })
    }
}
impl CatalogRepository for PgStore {
    fn health(&self) -> PortFuture<'_, ()> {
        Box::pin(async move {
            let result: Result<_> = async {
                sqlx::query("SELECT 1").execute(&self.pool).await?;
                Ok(())
            }
            .await;
            result.map_err(storage_error)
        })
    }
    fn corpus(&self) -> PortFuture<'_, CorpusSnapshot> {
        Box::pin(async move {
            let result: Result<_> = async {
        let embeddings=match &self.embeddings { Some(model)=>model.status(&self.pool).await?,None=>EmbeddingStatus::Disabled };
        let versions=sqlx::query_scalar::<_, sqlx::types::Json<CorpusVersion>>("SELECT jsonb_build_object('title',a.title,'number',a.number,'judgment',a.judgment,'treaty',a.treaty,'regulation',a.regulation,'valid_from',v.valid_from,'valid_to',v.valid_to,'verified_on',v.verified_on,'source_url',v.source_url,'pdf_url',v.pdf_url,'sha256',v.sha256,'nodes',count(n.id),'sections',count(n.id) FILTER(WHERE n.kind='section'),'paragraphs',count(n.id) FILTER(WHERE n.kind='paragraph'),'letters',count(n.id) FILTER(WHERE n.kind='letter'),'points',count(n.id) FILTER(WHERE n.kind IN ('point','judgment_point')),'articles',count(n.id) FILTER(WHERE n.kind='article')) FROM acts a JOIN act_versions v ON v.act_id=a.id JOIN legal_nodes n ON n.version_id=v.id GROUP BY a.id,v.id ORDER BY v.valid_from DESC").fetch_all(&self.pool).await?;
        Ok(CorpusSnapshot {versions: versions.into_iter().map(|row| row.0).collect(),embeddings})
        }.await;
            result.map_err(storage_error)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{usecases::agent::Answer, usecases::retrieval::Architecture};
    use chrono::NaiveDate;

    async fn unlocked_turn(
        store: &PgStore,
        owner: Uuid,
        conversation: Uuid,
    ) -> Box<dyn ConversationTurn> {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                match store.begin_turn(owner, conversation).await {
                    Ok(turn) => break turn,
                    Err(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
                }
            }
        })
        .await
        .expect("dropped transaction releases the conversation lock")
    }

    #[sqlx::test(migrations = "./migrations")]
    #[ignore = "requires disposable PostgreSQL via DATABASE_URL"]
    async fn conversation_turn_reads_only_its_current_documents_without_truncation(pool: PgPool) {
        let store = PgStore {
            pool,
            embeddings: None,
        };
        let owner = Uuid::new_v4();
        let conversation = Uuid::new_v4();
        let other_conversation = Uuid::new_v4();
        store.create_session(owner).await.unwrap();
        store.create(owner, conversation).await.unwrap();
        store.create(owner, other_conversation).await.unwrap();
        let text = format!("{}Údaj na konci dokumentu.", "Príloha 🧾. ".repeat(1000));
        let document = Uuid::new_v4();
        for (id, target, content) in [
            (document, conversation, text.as_str()),
            (Uuid::new_v4(), other_conversation, "Cudzí dokument."),
        ] {
            sqlx::query("INSERT INTO conversation_attachments(id,conversation_id,filename,content_type,size_bytes,text_chars,original_bytes,extracted_text) VALUES($1,$2,'priloha.txt','text/plain',$3,$4,$5,$6)")
                .bind(id)
                .bind(target)
                .bind(content.len() as i64)
                .bind(content.chars().count() as i64)
                .bind(content.as_bytes())
                .bind(content)
                .execute(&store.pool)
                .await
                .unwrap();
        }
        let mut turn = store.begin_turn(owner, conversation).await.unwrap();
        let attachments = turn.attachments().await.unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].id, document);
        assert_eq!(attachments[0].filename, "priloha.txt");
        assert_eq!(attachments[0].text, text);
        assert!(turn.history().await.unwrap().is_empty());
        drop(turn);
        sqlx::query("DELETE FROM conversation_attachments WHERE id=$1")
            .bind(document)
            .execute(&store.pool)
            .await
            .unwrap();
        let mut next = unlocked_turn(&store, owner, conversation).await;
        assert!(next.attachments().await.unwrap().is_empty());
    }

    #[sqlx::test(migrations = "./migrations")]
    #[ignore = "requires disposable PostgreSQL via DATABASE_URL"]
    async fn conversation_turn_enforces_ownership_locking_and_atomic_commit(pool: PgPool) {
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
        let error = store
            .begin_turn(stranger, conversation)
            .await
            .err()
            .unwrap();
        assert!(error.downcast_ref::<ApplicationError>().is_some());
        let turn = store.begin_turn(owner, conversation).await.unwrap();
        let error = store.begin_turn(owner, conversation).await.err().unwrap();
        assert!(error.downcast_ref::<StorageFailure>().is_some());
        drop(turn);

        // Force failure on the second INSERT, after the user message was inserted.
        sqlx::query("CREATE FUNCTION reject_answer() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.role='assistant' THEN RAISE EXCEPTION 'fixture rejection'; END IF; RETURN NEW; END $$")
            .execute(&store.pool).await.unwrap();
        sqlx::query("CREATE TRIGGER reject_answer BEFORE INSERT ON messages FOR EACH ROW EXECUTE FUNCTION reject_answer()")
            .execute(&store.pool).await.unwrap();
        let answer = RunAnswer {
            content: Answer {
                answer: "Rešerš".into(),
                sources: vec![],
                mode: "research".into(),
                as_of: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
                claims: vec![],
            },
            architecture: Architecture::Pipeline,
            tool_calls: vec![],
        };
        let turn = unlocked_turn(&store, owner, conversation).await;
        assert!(turn.commit("Otázka", &answer).await.is_err());
        assert!(
            store
                .messages(owner, conversation)
                .await
                .unwrap()
                .is_empty()
        );
        sqlx::query("DROP TRIGGER reject_answer ON messages")
            .execute(&store.pool)
            .await
            .unwrap();
        let mut turn = unlocked_turn(&store, owner, conversation).await;
        assert!(turn.history().await.unwrap().is_empty());
        turn.commit("Otázka", &answer).await.unwrap();
        let messages = store.messages(owner, conversation).await.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[1].role, MessageRole::Assistant);
        assert_eq!(messages[1].architecture, Some(Architecture::Pipeline));
        sqlx::query("INSERT INTO messages(conversation_id,role,content,as_of,mode) VALUES($1,'assistant','Historická odpoveď',$2,'research')")
            .bind(conversation).bind(answer.content.as_of).execute(&store.pool).await.unwrap();
        let historical = store.messages(owner, conversation).await.unwrap();
        assert!(historical[2].architecture.is_none());
        assert!(historical[2].tool_calls.is_empty());
        assert!(historical[2].claims.is_empty());
        assert!(historical[2].sources.is_empty());
        assert!(
            store
                .messages(stranger, conversation)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
