use crate::adapters::spi::{
    attachment_extractor::DocumentExtractor,
    db::{self, embedding_index::Embeddings, import},
    http::{embedding_client::EmbeddingClient, http_language_model as model},
};
use crate::application::{
    repositories::LanguageModel,
    usecases::chat_usecase::ChatService,
    usecases::retrieval::{Architecture, Config},
    usecases::services::Services,
};
use anyhow::{Context, Result, ensure};
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

fn rag_config() -> Result<Config> {
    let default = match std::env::var("RAG_ARCHITECTURE")
        .unwrap_or_else(|_| "agentic".into())
        .as_str()
    {
        "pipeline" => Architecture::Pipeline,
        "agentic" => Architecture::Agentic,
        _ => anyhow::bail!("RAG_ARCHITECTURE must be pipeline or agentic"),
    };
    let max_turns = std::env::var("RAG_MAX_TURNS")
        .unwrap_or_else(|_| "6".into())
        .parse()?;
    ensure!(
        (1..=12).contains(&max_turns),
        "RAG_MAX_TURNS must be between 1 and 12"
    );
    Ok(Config { default, max_turns })
}

pub async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lesana=info".into()),
        )
        .init();
    let args: Vec<String> = std::env::args().collect();
    let corpora: Vec<(&str, import::Manifest)> = import::BUNDLED_CORPORA
        .iter()
        .map(|(html, metadata)| Ok((*html, serde_json::from_str(metadata)?)))
        .collect::<Result<_>>()?;
    if args.get(1).map(String::as_str) == Some("check-corpus") {
        for (html, manifest) in &corpora {
            let nodes = import::parse(html, manifest)?;
            println!(
                "{}: {} nodes; {} sections; {} articles; {} letters; source SHA-256 verified",
                manifest.number,
                nodes.len(),
                manifest.expected_sections,
                manifest.expected_articles,
                manifest.expected_letters
            );
        }
        return Ok(());
    }
    let pool = db::db_connection::connect().await?;
    let embeddings = EmbeddingClient::from_env()?.map(Embeddings::new);
    if args.get(1).map(String::as_str) == Some("import") {
        anyhow::ensure!(
            args.len() == 4,
            "Usage: lesana import SOURCE.html MANIFEST.json"
        );
        let html = std::fs::read_to_string(&args[2])?;
        let metadata = serde_json::from_str(&std::fs::read_to_string(&args[3])?)?;
        let count = import::import(&pool, &html, &metadata).await?;
        println!("Imported {count} nodes");
        if let Some(model) = &embeddings {
            println!("Indexed {} nodes", model.backfill(&pool).await?);
        }
        return Ok(());
    }
    for (html, manifest) in &corpora {
        import::import(&pool, html, manifest).await?;
    }
    if args.get(1).map(String::as_str) == Some("embed") {
        let model = embeddings
            .context("Set EMBEDDING_BASE_URL and EMBEDDING_MODEL before running lesana embed")?;
        println!("Indexed {} nodes", model.backfill(&pool).await?);
        return Ok(());
    }
    if let Some(model) = &embeddings {
        model.start_worker(pool.clone());
    }
    let model = model::Model::from_env()?;
    let model_name = model.as_ref().map(|model| model.name.clone());
    let legal = Arc::new(db::PgLegalRepository::new(pool.clone(), embeddings.clone()));
    let source_requests = Arc::new(db::source_requests::PgSourceRequests { pool: pool.clone() });
    source_requests.start_worker();
    let store = Arc::new(db::PgStore { pool, embeddings });
    let services = Arc::new(
        Services::new(
            ChatService::new(
                legal,
                model.map(|model| Arc::new(model) as Arc<dyn LanguageModel>),
                store.clone(),
                rag_config()?,
            ),
            store.clone(),
            store.clone(),
        )
        .with_source_requests(source_requests)
        .with_attachments(store, Arc::new(DocumentExtractor)),
    );
    let routes = crate::adapters::api::router(crate::adapters::api::App {
        services,
        model_name,
        password: Arc::new(std::env::var("APP_PASSWORD").unwrap_or_default()),
        secure_cookie: std::env::var("COOKIE_SECURE").unwrap_or_default() == "true",
        chat_slots: Arc::new(Semaphore::new(2)),
        chat_runs: Default::default(),
        login_attempts: Arc::new(Mutex::new(vec![])),
    });
    let address = std::env::var("LISTEN_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8088".into());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!(%address,"Lesana ready");
    axum::serve(listener, routes)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _=ctrl_c=>{}, _=terminate=>{} }
}
