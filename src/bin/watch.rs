use anyhow::Result;
use reqwest;
use updown::Database;
use updown::{models::Response, Site};

extern crate updown;

async fn db() -> Database {
    let database_url = std::env::var("DATABASE_URL").unwrap();
    Database::new(database_url).await
}

#[tokio::main]
async fn main() {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));

    loop {
        interval.tick().await;
        tokio::spawn(async {
            _ = monitor().await;
        });
    }
}

async fn monitor() -> Result<()> {
    let sites = db().await.sites().await?;
    for site in sites {
        let response = response(&site).await?;
        db().await.upsert_response(response).await?;
    }
    Ok(())
}

async fn response<'a>(site: &'a Site) -> Result<Response> {
    let status_code: i64 = reqwest::get(&site.url).await?.status().as_u16() as i64;
    let mut res = Response::default();
    res.status_code = status_code;
    res.site_id = site.id;
    Ok(res)
}
