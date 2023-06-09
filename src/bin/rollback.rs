use anyhow::Result;
use updown::Database;

extern crate updown;

#[tokio::main]
async fn main() -> Result<()> {
    let database_url = std::env::var("DATABASE_URL")?;
    let db = Database::new(database_url).await;
    db.rollback().await?;
    Ok(())
}
