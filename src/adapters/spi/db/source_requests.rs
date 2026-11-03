//! Persistent, leased import queue. HTTP requests only enqueue work.
use crate::{
    adapters::spi::http::requested_source::{self, ImportOutcome},
    application::{
        contracts::SourceRequest,
        repositories::{PortFuture, SourceRequestRepository},
        utils::errors::ApplicationError,
    },
};
use anyhow::Result;
use sqlx::{PgPool, Row, postgres::PgRow};
use std::{future::Future, time::Duration};
use uuid::Uuid;

#[derive(Clone)]
pub struct PgSourceRequests {
    pub pool: PgPool,
}

fn record(row: PgRow) -> Result<SourceRequest> {
    Ok(SourceRequest {
        id: row.try_get("id")?,
        kind: row.try_get("kind")?,
        reference: row.try_get("reference")?,
        status: row.try_get("status")?,
        message: row.try_get("message")?,
        act_id: row.try_get("act_id")?,
        title: row.try_get("title")?,
        node_count: row.try_get("node_count")?,
        imported_count: row.try_get("imported_count")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

impl SourceRequestRepository for PgSourceRequests {
    fn enqueue<'a>(
        &'a self,
        owner: Uuid,
        kind: &'a str,
        reference: &'a str,
    ) -> PortFuture<'a, SourceRequest> {
        Box::pin(async move {
            let reference = requested_source::validate_reference(kind, reference).map_err(|_| {
                ApplicationError::InvalidInput("Zadajte číslo predpisu alebo odkaz Slov-Lex, CELEX/odkaz EUR-Lex alebo priamy PDF odkaz na rozhodnutie či zbierku na www.nsud.sk, alebo odkaz na rozhodnutie NS ČR z rozhodnuti.nsoud.cz podľa zvoleného typu zdroja.")
            })?;
            let mut tx = self.pool.begin().await?;
            // Serialize deduplication, subscriptions and capacity checks across replicas.
            sqlx::query("SELECT pg_advisory_xact_lock(1622015009)")
                .execute(&mut *tx)
                .await?;
            let existing = sqlx::query(
                "SELECT * FROM source_import_jobs WHERE kind=$1 AND reference=$2 FOR UPDATE",
            )
            .bind(kind)
            .bind(&reference)
            .fetch_optional(&mut *tx)
            .await?
            .map(record)
            .transpose()?;
            // A bare act number denotes the version effective today. A completed
            // request must resolve that mutable reference again on an explicit POST.
            let needs_work = existing.as_ref().is_none_or(|job| {
                job.status == "failed"
                    || (job.status == "completed"
                        && kind == "legislation"
                        && reference.ends_with('/'))
            });
            if needs_work {
                let pending: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM source_import_jobs WHERE status IN ('queued','running')",
                )
                .fetch_one(&mut *tx)
                .await?;
                let owned: i64 = sqlx::query_scalar("SELECT count(*) FROM source_requests r JOIN source_import_jobs j ON j.id=r.job_id WHERE r.session_id=$1 AND j.status IN ('queued','running')")
                    .bind(owner).fetch_one(&mut *tx).await?;
                if pending >= 100 || owned >= 5 {
                    return Err(ApplicationError::InvalidInput(
                        "Prebieha príliš veľa importov. Počkajte na dokončenie a skúste znova.",
                    )
                    .into());
                }
            }
            let id = existing
                .as_ref()
                .map(|job| job.id)
                .unwrap_or_else(Uuid::new_v4);
            if existing.is_none() {
                sqlx::query("INSERT INTO source_import_jobs(id,kind,reference) VALUES($1,$2,$3)")
                    .bind(id)
                    .bind(kind)
                    .bind(&reference)
                    .execute(&mut *tx)
                    .await?;
            } else if needs_work {
                sqlx::query("UPDATE source_import_jobs SET status='queued',message=NULL,act_id=NULL,title=NULL,node_count=NULL,imported_count=NULL,lease_until=NULL,attempt_token=NULL,updated_at=now() WHERE id=$1")
                    .bind(id).execute(&mut *tx).await?;
            }
            sqlx::query("INSERT INTO source_requests(session_id,job_id) VALUES($1,$2) ON CONFLICT(session_id,job_id) DO UPDATE SET created_at=now()")
                .bind(owner).bind(id).execute(&mut *tx).await?;
            let result = record(
                sqlx::query("SELECT * FROM source_import_jobs WHERE id=$1")
                    .bind(id)
                    .fetch_one(&mut *tx)
                    .await?,
            )?;
            tx.commit().await?;
            Ok(result)
        })
    }

    fn list(&self, owner: Uuid) -> PortFuture<'_, Vec<SourceRequest>> {
        Box::pin(async move {
            sqlx::query("SELECT j.* FROM source_import_jobs j JOIN source_requests r ON r.job_id=j.id WHERE r.session_id=$1 ORDER BY (j.status IN ('queued','running')) DESC,r.created_at DESC LIMIT 50")
                .bind(owner).fetch_all(&self.pool).await?.into_iter().map(record).collect()
        })
    }
}

impl PgSourceRequests {
    pub fn start_worker(&self) {
        let store = self.clone();
        tokio::spawn(async move {
            loop {
                let pool = store.pool.clone();
                let worked = store
                    .process_next(|kind, reference| async move {
                        requested_source::fetch_import(&pool, &kind, &reference).await
                    })
                    .await;
                match worked {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => tracing::error!(%error, "Source import worker failed"),
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    async fn claim(&self) -> Result<Option<(SourceRequest, Uuid)>> {
        let token = Uuid::new_v4();
        // A 25-minute lease exceeds the 20-minute OCR/import timeout. A crashed
        // process cannot strand work; another worker can reclaim an expired lease.
        let row = sqlx::query("WITH candidate AS (SELECT id FROM source_import_jobs WHERE status='queued' OR (status='running' AND lease_until < now()) ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1) UPDATE source_import_jobs j SET status='running',message='Načítavam a overujem zdroj. Skenovaná zbierka môže vyžadovať niekoľko minút.',attempt_token=$1,lease_until=now()+interval '25 minutes',updated_at=now() FROM candidate c WHERE j.id=c.id RETURNING j.*")
            .bind(token).fetch_optional(&self.pool).await?;
        row.map(|row| Ok((record(row)?, token))).transpose()
    }

    async fn finish(&self, id: Uuid, token: Uuid, result: Result<ImportOutcome>) -> Result<()> {
        match result {
            Ok(imported) => {
                sqlx::query("UPDATE source_import_jobs SET status='completed',message='Import je dokončený a všetky záznamy sú dostupné vo vyhľadávaní.',act_id=$3,title=$4,node_count=$5,imported_count=$6,lease_until=NULL,attempt_token=NULL,updated_at=now() WHERE id=$1 AND attempt_token=$2 AND status='running'")
                    .bind(id).bind(token).bind(imported.act_id).bind(imported.title).bind(imported.node_count as i64).bind(imported.imported_count as i64).execute(&self.pool).await?;
            }
            Err(error) => {
                tracing::warn!(%id, %error, "Requested source import failed");
                let message = error.downcast_ref::<requested_source::SourceValidationError>()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "Zdroj sa nepodarilo načítať alebo overiť jeho úplné znenie a metadáta. Skontrolujte označenie a podporovaný formát zdroja, potom skúste znova.".into());
                sqlx::query("UPDATE source_import_jobs SET status='failed',message=$3,lease_until=NULL,attempt_token=NULL,updated_at=now() WHERE id=$1 AND attempt_token=$2 AND status='running'")
                    .bind(id).bind(token).bind(message).execute(&self.pool).await?;
            }
        }
        Ok(())
    }

    async fn process_next<F, Fut>(&self, import: F) -> Result<bool>
    where
        F: FnOnce(String, String) -> Fut,
        Fut: Future<Output = Result<ImportOutcome>>,
    {
        let Some((job, token)) = self.claim().await? else {
            return Ok(false);
        };
        let result =
            match tokio::time::timeout(Duration::from_secs(1200), import(job.kind, job.reference))
                .await
            {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!("Import prekročil časový limit 20 minút.")),
            };
        self.finish(job.id, token, result).await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[sqlx::test(migrations = "./migrations")]
    #[ignore = "requires PostgreSQL"]
    async fn czech_import_queue_retrieval_and_supporting_metadata(pool: PgPool) {
        use crate::adapters::spi::db::{czech_supreme_court, import, search};
        let a = owner(&pool).await;
        let b = owner(&pool).await;
        let store = PgSourceRequests { pool: pool.clone() };
        let kind = "czech_supreme_court_judgment";
        let source = "https://rozhodnuti.nsoud.cz/Judikatura/judikatura_ns.nsf/WebSearch/A4D79508F15A7C04C1258A37004CF0D1?openDocument";
        let html = include_str!("../../../../tests/fixtures/nscr-33cdo314-2023.html");
        let date = chrono::NaiveDate::from_ymd_opt(2026, 10, 13).unwrap();
        let m = czech_supreme_court::manifest(html, source, date).unwrap();
        let job = store.enqueue(a, kind, source).await.unwrap();
        assert!(store.list(b).await.unwrap().is_empty());
        assert_eq!(
            store
                .enqueue(
                    b,
                    kind,
                    &source.replace("rozhodnuti.nsoud.cz", "www.nsoud.cz")
                )
                .await
                .unwrap()
                .id,
            job.id
        );
        store
            .process_next(|_, _| async {
                let node_count = import::import(&pool, html, &m).await?;
                Ok(ImportOutcome {
                    act_id: m.act_id.clone(),
                    title: m.title.clone(),
                    node_count,
                    imported_count: 1,
                })
            })
            .await
            .unwrap();
        let result = store.list(a).await.unwrap().remove(0);
        assert_eq!(result.status, "completed");
        assert_eq!(result.imported_count, Some(1));
        assert_eq!(
            store.enqueue(a, kind, source).await.unwrap().status,
            "completed"
        );
        assert_eq!(
            import::import(&pool, html, &m).await.unwrap() as i64,
            result.node_count.unwrap()
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM act_versions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        for (query, anchor) in [
            ("33 Cdo 314/2023", "judgment"),
            ("33 Cdo 314/2023 výrok", "judgment-operative"),
            (
                "ECLI:CZ:NS:2023:33.CDO.314.2023.1 odôvodnenie",
                "judgment-reasoning",
            ),
        ] {
            let found = search::search(&pool, None, query, date, 6).await.unwrap();
            assert_eq!(found.len(), 1, "{query}");
            assert_eq!(found[0].anchor, anchor);
            assert!(found[0].judgment.as_ref().unwrap().supporting_only);
            assert_eq!(found[0].source_url, source);
            assert!(found[0].pdf_url.is_empty());
            let stored = search::get_node(&pool, found[0].id).await.unwrap().unwrap();
            assert!(stored.judgment.unwrap().supporting_only);
        }
        for query in [
            "33 Cdo 315/2023",
            "33 Cdo 314/2023 strana 2",
            "33 Cdo 314/2023 bod 2",
        ] {
            assert!(
                search::search(&pool, None, query, date, 6)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        let before = chrono::NaiveDate::from_ymd_opt(2023, 8, 16).unwrap();
        assert!(
            search::search(&pool, None, "33 Cdo 314/2023", before, 6)
                .await
                .unwrap()
                .is_empty()
        );
        let found = search::get_judgment(&pool, "33 Cdo 314/2023", date)
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].text.contains("Poučení:"));
        let found = search::search_kind(
            &pool,
            None,
            "narovnáním",
            date,
            6,
            crate::domain::SourceKind::Judgments,
        )
        .await
        .unwrap();
        assert!(!found.is_empty());
        assert!(
            found
                .iter()
                .all(|s| s.judgment.as_ref().unwrap().supporting_only)
        );
        assert!(
            search::search_kind(
                &pool,
                None,
                "narovnáním",
                date,
                6,
                crate::domain::SourceKind::Legislation
            )
            .await
            .unwrap()
            .is_empty()
        );
    }
    #[sqlx::test(migrations = "./migrations")]
    #[ignore = "requires PostgreSQL"]
    async fn entire_collection_is_searchable_and_retry_keeps_counts(pool: PgPool) {
        use crate::adapters::spi::db::{import, supreme_court_collection};
        let a = owner(&pool).await;
        let store = PgSourceRequests { pool: pool.clone() };
        let source = "https://www.nsud.sk/data/att/67c/688261.8f5b7b.pdf";
        let pages = include_str!("../../../../tests/fixtures/nssr-zbierka-2002-2.txt")
            .trim_end()
            .trim_end_matches('\u{000c}')
            .split('\u{000c}')
            .map(|page| page.trim().to_owned())
            .collect();
        let date = chrono::NaiveDate::from_ymd_opt(2026, 10, 10).unwrap();
        let documents =
            supreme_court_collection::snapshot_manifests(pages, "a".repeat(64), source, date, true)
                .unwrap();
        assert_eq!(documents.len(), 16);
        store
            .enqueue(a, "supreme_court_collection", source)
            .await
            .unwrap();
        store
            .process_next(|_, _| async {
                let count = import::import_batch(&pool, &documents).await?;
                Ok(ImportOutcome {
                    act_id: documents[0].1.act_id.clone(),
                    title: "Zbierka 2/2002".into(),
                    node_count: count,
                    imported_count: documents.len(),
                })
            })
            .await
            .unwrap();
        let completed = store.list(a).await.unwrap().remove(0);
        assert_eq!(completed.status, "completed");
        assert_eq!(completed.imported_count, Some(16));
        for number in 20..=35 {
            let found =
                super::super::search::search(&pool, None, &format!("R {number}/2002"), date, 6)
                    .await
                    .unwrap();
            assert_eq!(found.len(), 1, "R {number}/2002");
            assert_eq!(found[0].kind, "judgment");
            assert!(
                found[0]
                    .judgment
                    .as_ref()
                    .unwrap()
                    .collection
                    .as_ref()
                    .unwrap()
                    .ocr
            );
        }
        let repeated = store
            .enqueue(a, "supreme_court_collection", source)
            .await
            .unwrap();
        assert_eq!(repeated.status, "completed");
        assert_eq!(repeated.imported_count, Some(16));
        assert_eq!(
            import::import_batch(&pool, &documents).await.unwrap() as i64,
            completed.node_count.unwrap()
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM acts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 16);
    }
    async fn owner(pool: &PgPool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO sessions(id) VALUES($1)")
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
        id
    }
    #[sqlx::test(migrations = "./migrations")]
    #[ignore = "requires PostgreSQL"]
    async fn deduplication_and_private_subscriptions(pool: PgPool) {
        let a = owner(&pool).await;
        let b = owner(&pool).await;
        let store = PgSourceRequests { pool };
        let first = store.enqueue(a, "judgment", "62001CJ0464").await.unwrap();
        assert_eq!(first.status, "queued");
        assert!(store.list(b).await.unwrap().is_empty());
        let same = store.enqueue(a, "judgment", "62001CJ0464").await.unwrap();
        assert_eq!(first.id, same.id);
        assert_eq!(store.list(a).await.unwrap().len(), 1);
        let shared = store.enqueue(b, "judgment", "62001CJ0464").await.unwrap();
        assert_eq!(first.id, shared.id);
    }
    #[sqlx::test(migrations = "./migrations")]
    #[ignore = "requires PostgreSQL"]
    async fn failed_jobs_retry_and_crashed_leases_recover(pool: PgPool) {
        let a = owner(&pool).await;
        let store = PgSourceRequests { pool };
        let first = store.enqueue(a, "judgment", "62001CJ0464").await.unwrap();
        let (old, old_token) = store.claim().await.unwrap().unwrap();
        assert_eq!(old.id, first.id);
        assert!(store.claim().await.unwrap().is_none());
        sqlx::query(
            "UPDATE source_import_jobs SET lease_until=now()-interval '1 second' WHERE id=$1",
        )
        .bind(first.id)
        .execute(&store.pool)
        .await
        .unwrap();
        let (_, new_token) = store.claim().await.unwrap().unwrap();
        assert_ne!(old_token, new_token);
        store
            .finish(first.id, old_token, Err(anyhow::anyhow!("stale worker")))
            .await
            .unwrap();
        assert_eq!(store.list(a).await.unwrap()[0].status, "running");
        store
            .finish(
                first.id,
                new_token,
                Err(anyhow::anyhow!("unsupported source")),
            )
            .await
            .unwrap();
        assert_eq!(store.list(a).await.unwrap()[0].status, "failed");
        let retried = store.enqueue(a, "judgment", "62001CJ0464").await.unwrap();
        assert_eq!(retried.id, first.id);
        assert_eq!(retried.status, "queued");
        assert!(retried.message.is_none());
    }

    #[sqlx::test(migrations = "./migrations")]
    #[ignore = "requires PostgreSQL"]
    async fn completed_current_law_is_resolved_again_but_fixed_sources_are_reused(pool: PgPool) {
        let a = owner(&pool).await;
        let store = PgSourceRequests { pool };
        for (kind, reference, expected_status) in [
            ("legislation", "311/2001", "queued"),
            (
                "legislation",
                "https://static.slov-lex.sk/static/SK/ZZ/2001/311/20260901.html",
                "completed",
            ),
            ("judgment", "62008CJ0243", "completed"),
        ] {
            let first = store.enqueue(a, kind, reference).await.unwrap();
            sqlx::query("UPDATE source_import_jobs SET status='completed',title='Previous version',node_count=1 WHERE id=$1")
                .bind(first.id).execute(&store.pool).await.unwrap();
            let next = store.enqueue(a, kind, reference).await.unwrap();
            assert_eq!(next.id, first.id);
            assert_eq!(next.status, expected_status);
            if expected_status == "queued" {
                assert!(next.title.is_none());
                assert!(next.node_count.is_none());
            }
        }
    }
    #[sqlx::test(migrations = "./migrations")]
    #[ignore = "requires PostgreSQL"]
    async fn successful_worker_import_is_immediately_searchable(pool: PgPool) {
        let a = owner(&pool).await;
        let store = PgSourceRequests { pool: pool.clone() };
        let manifest: super::super::import::Manifest =
            serde_json::from_str(super::super::import::BUNDLED_MANIFEST).unwrap();
        store
            .enqueue(a, "legislation", &manifest.source_url)
            .await
            .unwrap();
        store
            .process_next(|_, _| async {
                let count = super::super::import::import(
                    &pool,
                    super::super::import::BUNDLED_HTML,
                    &manifest,
                )
                .await?;
                Ok(ImportOutcome {
                    act_id: manifest.act_id.clone(),
                    title: manifest.title.clone(),
                    node_count: count,
                    imported_count: 1,
                })
            })
            .await
            .unwrap();
        assert_eq!(store.list(a).await.unwrap()[0].status, "completed");
        let found =
            super::super::search::get_section(&pool, "181", manifest.verified_on, Some("162/2015"))
                .await
                .unwrap();
        assert!(!found.is_empty());
        assert!(
            !store
                .process_next(|_, _| async { anyhow::bail!("must not repeat") })
                .await
                .unwrap()
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    #[ignore = "requires PostgreSQL"]
    async fn concurrent_submissions_deduplicate_and_pending_work_is_bounded(pool: PgPool) {
        let a = owner(&pool).await;
        let store = PgSourceRequests { pool };
        let (one, two) = tokio::join!(
            store.enqueue(a, "judgment", "62001CJ0464"),
            store.enqueue(a, "judgment", "62001CJ0464")
        );
        assert_eq!(one.unwrap().id, two.unwrap().id);
        for reference in ["62001CJ0465", "62001CJ0466", "62001CJ0467", "62001CJ0468"] {
            store.enqueue(a, "judgment", reference).await.unwrap();
        }
        assert!(store.enqueue(a, "judgment", "62001CJ0469").await.is_err());
        // Retrying a request already queued remains idempotent at capacity.
        store.enqueue(a, "judgment", "62001CJ0464").await.unwrap();
        assert_eq!(store.list(a).await.unwrap().len(), 5);
    }
}
