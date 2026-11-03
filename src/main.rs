#[tokio::main]
async fn main() -> anyhow::Result<()> {
    lesana::infrastructure::run().await
}
