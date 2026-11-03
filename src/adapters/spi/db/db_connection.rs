use anyhow::{Context, Result};
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::time::Duration;
pub async fn connect() -> Result<PgPool> {
    let options = if let Ok(url) = std::env::var("DATABASE_URL") {
        url.parse::<PgConnectOptions>()?
    } else {
        PgConnectOptions::new()
    };
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(options)
        .await
        .context("Connecting to PostgreSQL")?;
    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
}
