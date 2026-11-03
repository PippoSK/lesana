//! Transactional PostgreSQL embedding index and background worker.
use crate::adapters::spi::http::embedding_client::EmbeddingClient;
use crate::application::contracts::EmbeddingStatus;
use anyhow::{Result, ensure};
use sqlx::{FromRow, PgPool};
use std::time::Duration;
const BATCH_SIZE: usize = 16;
#[derive(Clone)]
pub struct Embeddings {
    client: EmbeddingClient,
}
impl Embeddings {
    pub fn new(client: EmbeddingClient) -> Self {
        Self { client }
    }
    pub fn key(&self) -> &str {
        &self.client.key
    }
    pub fn min_similarity(&self) -> f64 {
        self.client.min_similarity
    }
    pub async fn query_vector(&self, text: &str, dimensions: i32) -> Result<String> {
        self.client.query_vector(text, dimensions).await
    }
    async fn backfill_batch(&self, pool: &PgPool) -> Result<Option<usize>> {
        #[derive(FromRow)]
        struct Document {
            node_id: i64,
            content: String,
            content_hash: String,
        }
        let mut tx = pool.begin().await?;
        let locked: bool =
            sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 7102026))")
                .bind(&self.client.key)
                .fetch_one(&mut *tx)
                .await?;
        if !locked {
            return Ok(None);
        }
        let documents: Vec<Document> = sqlx::query_as(
            "SELECT i.node_id,i.content,i.content_hash FROM legal_embedding_inputs i
             LEFT JOIN legal_embedding_documents d ON d.node_id=i.node_id AND d.model_key=$1
             WHERE d.node_id IS NULL OR d.content_hash<>i.content_hash ORDER BY i.node_id LIMIT $2",
        )
        .bind(&self.client.key)
        .bind(BATCH_SIZE as i64)
        .fetch_all(&mut *tx)
        .await?;
        if documents.is_empty() {
            tx.commit().await?;
            return Ok(Some(0));
        }
        let mut inputs = Vec::new();
        let mut owners = Vec::new();
        for doc in &documents {
            for (index, chunk) in self
                .client
                .document_inputs(&doc.content)
                .into_iter()
                .enumerate()
            {
                inputs.push(chunk);
                owners.push((doc.node_id, index as i32));
            }
        }
        let mut vectors = Vec::new();
        for batch in inputs.chunks(BATCH_SIZE) {
            vectors.extend(self.client.embed(batch).await?);
        }
        let dimensions = vectors[0].len() as i32;
        ensure!(
            vectors.iter().all(|v| v.len() == dimensions as usize),
            "Embedding dimensions changed between batches"
        );
        sqlx::query("INSERT INTO embedding_models(model_key,model_name,dimensions) VALUES($1,$2,$3) ON CONFLICT DO NOTHING")
            .bind(&self.client.key).bind(&self.client.name).bind(dimensions).execute(&mut *tx).await?;
        let stored: i32 =
            sqlx::query_scalar("SELECT dimensions FROM embedding_models WHERE model_key=$1")
                .bind(&self.client.key)
                .fetch_one(&mut *tx)
                .await?;
        ensure!(
            stored == dimensions,
            "Embedding model dimensions changed; set a new EMBEDDING_REVISION and reindex"
        );
        for doc in &documents {
            // Replace every chunk and its fingerprint atomically. Failed batches
            // leave the previous index intact and can be resumed after restart.
            sqlx::query("DELETE FROM legal_embedding_documents WHERE model_key=$1 AND node_id=$2")
                .bind(&self.client.key)
                .bind(doc.node_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO legal_embedding_documents(model_key,node_id,content_hash) VALUES($1,$2,$3)")
                .bind(&self.client.key).bind(doc.node_id).bind(&doc.content_hash).execute(&mut *tx).await?;
        }
        for ((node_id, index), vector) in owners.into_iter().zip(vectors) {
            sqlx::query("INSERT INTO legal_node_embeddings(model_key,node_id,chunk_index,dimensions,embedding) VALUES($1,$2,$3,$4,$5::text::vector)")
                .bind(&self.client.key).bind(node_id).bind(index).bind(dimensions)
                .bind(serde_json::to_string(&vector)?).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(Some(documents.len()))
    }

    pub async fn backfill(&self, pool: &PgPool) -> Result<usize> {
        let mut total = 0;
        loop {
            match self.backfill_batch(pool).await? {
                Some(0) => return Ok(total),
                Some(count) => {
                    total += count;
                    tracing::info!(indexed = total, "Embedding backfill progress");
                }
                None => tokio::time::sleep(Duration::from_secs(1)).await,
            }
        }
    }

    pub fn start_worker(&self, pool: PgPool) {
        let model = self.clone();
        tokio::spawn(async move {
            loop {
                if let Err(error) = model.backfill(&pool).await {
                    tracing::warn!(%error, "Embedding backfill failed; retrying in 60 seconds");
                }
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
    }

    pub async fn status(&self, pool: &PgPool) -> Result<EmbeddingStatus> {
        let (total, indexed): (i64, i64) = sqlx::query_as(
            "SELECT count(*),count(d.node_id) FROM legal_embedding_inputs i
             LEFT JOIN legal_embedding_documents d ON d.node_id=i.node_id AND d.model_key=$1 AND d.content_hash=i.content_hash")
            .bind(&self.client.key).fetch_one(pool).await?;
        Ok(EmbeddingStatus::Enabled {
            model: self.client.name.clone(),
            indexed_nodes: indexed,
            total_nodes: total,
            ready: total > 0 && total == indexed,
        })
    }
}
