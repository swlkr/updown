use anyhow::Result;
use nanoid::nanoid;
use serde::{Deserialize, Serialize};
use sqlx::{
    migrate::MigrateError,
    sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteQueryResult,
        SqliteSynchronous,
    },
    FromRow, SqlitePool,
};
use std::{
    fmt::Display,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AppError {
    Migrate,
    DatabaseInsert,
    Login,
    JsonParse,
    DatabaseSelect,
    UrlEmpty,
    Rollback,
}

impl From<MigrateError> for AppError {
    fn from(_value: MigrateError) -> Self {
        AppError::Migrate
    }
}

impl Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{:?}", self))
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }

    fn cause(&self) -> Option<&dyn std::error::Error> {
        self.source()
    }
}

#[derive(Serialize, Deserialize, Default, Clone, PartialEq, FromRow, Debug)]
pub struct User {
    pub id: i64,
    pub login_code: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize, Default, Clone, PartialEq, FromRow, Debug)]
pub struct Login {
    pub id: i64,
    pub user_id: i64,
    pub created_at: i64,
}

#[derive(Serialize, Deserialize, Default, Clone, PartialEq, FromRow, Debug)]
pub struct Site {
    pub id: i64,
    pub user_id: i64,
    pub url: String,
    pub name: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub mod models {
    use serde::{Deserialize, Serialize};
    use sqlx::FromRow;

    #[derive(Serialize, Deserialize, Default, Clone, PartialEq, FromRow, Debug)]
    pub struct Response {
        pub id: i64,
        pub status_code: i64,
        pub site_id: i64,
        pub created_at: i64,
        pub updated_at: i64,
    }
}

#[derive(Debug)]
pub struct Database {
    connection: SqlitePool,
}

impl Database {
    pub async fn new(filename: String) -> Self {
        Self {
            connection: Self::pool(&filename).await,
        }
    }

    pub async fn migrate(&self) -> Result<(), AppError> {
        sqlx::migrate!()
            .run(&self.connection)
            .await
            .map_err(|_| AppError::Migrate)
    }

    pub async fn rollback(&self) -> Result<SqliteQueryResult, AppError> {
        let migrations = sqlx::migrate!()
            .migrations
            .iter()
            .filter(|m| m.migration_type.is_down_migration());
        if let Some(migration) = migrations.last() {
            if migration.migration_type.is_down_migration() {
                let version = migration.version;
                match sqlx::query(&migration.sql)
                    .execute(&self.connection)
                    .await
                    .map_err(|_| AppError::Rollback)
                {
                    Ok(_) => sqlx::query("delete from _sqlx_migrations where version = ?")
                        .bind(version)
                        .execute(&self.connection)
                        .await
                        .map_err(|_| AppError::Rollback),
                    Err(_) => Err(AppError::Rollback),
                }
            } else {
                Err(AppError::Rollback)
            }
        } else {
            Err(AppError::Rollback)
        }
    }

    fn connection_options(filename: &str) -> SqliteConnectOptions {
        let options: SqliteConnectOptions = filename.parse().unwrap();
        options
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(30))
    }

    async fn pool(filename: &str) -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(Self::connection_options(filename))
            .await
            .unwrap()
    }

    fn now() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("unable to get epoch in insert_user")
            .as_secs_f64()
    }

    pub async fn insert_user(&self) -> Result<User, AppError> {
        let login_code = nanoid!();
        let now = Self::now();
        sqlx::query_as!(
            User,
            "insert into users (login_code, created_at, updated_at) values (?, ?, ?) returning *",
            login_code,
            now,
            now
        )
        .fetch_one(&self.connection)
        .await
        .map_err(|_| AppError::DatabaseInsert)
    }

    pub async fn user_by_id(&self, id: i64) -> Result<User, sqlx::Error> {
        sqlx::query_as!(User, "select * from users where id = ?", id)
            .fetch_one(&self.connection)
            .await
    }

    pub async fn user_by_login_code(&self, login_code: String) -> Result<User, sqlx::Error> {
        sqlx::query_as!(
            User,
            "select * from users where login_code = ? limit 1",
            login_code
        )
        .fetch_one(&self.connection)
        .await
    }

    pub async fn insert_login(&self, new_login: Login) -> Result<Login, sqlx::Error> {
        let now = Self::now();
        sqlx::query_as!(
            Login,
            "insert into logins (user_id, created_at) values (?, ?) returning *",
            new_login.user_id,
            now
        )
        .fetch_one(&self.connection)
        .await
    }

    pub async fn insert_site(&self, site: Site) -> Result<Site, sqlx::Error> {
        let now = Self::now();
        sqlx::query_as!(
            Site,
            "insert into sites (url, user_id, created_at, updated_at) values (?, ?, ?, ?) returning *",
            site.url,
            site.user_id,
            now,
            now,
        )
        .fetch_one(&self.connection)
        .await
    }

    pub async fn sites_by_user_id(&self, user_id: i64) -> Result<Vec<Site>, sqlx::Error> {
        sqlx::query_as!(Site, "select * from sites where user_id = ?", user_id,)
            .fetch_all(&self.connection)
            .await
    }

    pub async fn sites(&self) -> Result<Vec<Site>, sqlx::Error> {
        sqlx::query_as!(Site, "select * from sites")
            .fetch_all(&self.connection)
            .await
    }

    pub async fn upsert_response(
        &self,
        response: models::Response,
    ) -> Result<models::Response, sqlx::Error> {
        let now = Self::now();
        sqlx::query_as!(models::Response, r#"insert into responses (status_code, site_id, created_at, updated_at) values (?, ?, ?, ?) on conflict (status_code, site_id) do update set updated_at = ? returning *"#, response.status_code, response.site_id, now, now, now).fetch_one(&self.connection).await
    }
}
