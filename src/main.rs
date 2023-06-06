#![allow(non_snake_case)]

use anyhow::Result;
use dioxus::prelude::*;
use dioxus_liveview::LiveViewPool;
use nanoid::nanoid;
use salvo::{
    affix, handler,
    hyper::header::ORIGIN,
    prelude::{StatusError, TcpListener},
    writer::Text,
    ws::WebSocketUpgrade,
    Depot, Request, Response, Router, Server,
};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool,
};
use std::{
    net::SocketAddr,
    sync::{Arc, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[tokio::main]
async fn main() -> Result<()> {
    hot_reload_init!();
    ENV.set(Env::new()).unwrap();
    DB.set(Database::new(env().database_url.clone()).await)
        .unwrap();
    db().migrate().await.expect("migrations failed to run");
    let addr: SocketAddr = env().host.parse()?;

    println!("Listening on {}", addr);

    Server::new(TcpListener::bind(addr)).serve(routes()).await;

    return Ok(());
}

static ENV: OnceLock<Env> = OnceLock::new();
static DB: OnceLock<Database> = OnceLock::new();

#[derive(Debug)]
enum AppError {
    MigrateError(sqlx::migrate::MigrateError),
    InsertUser(sqlx::Error),
}

#[derive(Debug)]
struct Env {
    pub database_url: String,
    pub host: String,
    pub origin: String,
    pub ws_host: String,
    pub app_env: String,
}

impl Env {
    fn new() -> Self {
        let database_url = std::env::var("DATABASE_URL").unwrap();
        let host = std::env::var("HOST").unwrap();
        let origin = std::env::var("ORIGIN").unwrap();
        let ws_host = std::env::var("WS_HOST").unwrap();
        let app_env = std::env::var("APP_ENV").unwrap();
        Self {
            database_url,
            host,
            origin,
            ws_host,
            app_env,
        }
    }
}

fn env() -> &'static Env {
    ENV.get().expect("env is not initialized")
}

fn db() -> &'static Database {
    DB.get().expect("db is not initialized")
}

#[derive(Debug)]
struct Database {
    connection: SqlitePool,
}

impl Database {
    async fn new(filename: String) -> Self {
        Self {
            connection: Self::pool(&filename).await,
        }
    }

    async fn migrate(&self) -> Result<(), AppError> {
        sqlx::migrate!()
            .run(&self.connection)
            .await
            .map_err(|e| AppError::MigrateError(e))
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

    async fn insert_user(&self) -> Result<User, AppError> {
        let login_code = nanoid!();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("unable to get epoch in insert_user")
            .as_secs_f64();
        sqlx::query_as!(
            User,
            "insert into users (login_code, created_at, updated_at) values (?, ?, ?) returning *",
            login_code,
            now,
            now
        )
        .fetch_one(&self.connection)
        .await
        .map_err(|e| AppError::InsertUser(e))
    }
}

#[derive(Clone)]
struct User {
    id: i64,
    login_code: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Clone, PartialEq)]
struct Site {
    url: String,
}

fn at(path: &str) -> salvo::Router {
    Router::with_path(path)
}

fn routes() -> Router {
    let view = LiveViewPool::new();
    let arc_view = Arc::new(view);
    Router::new()
        .hoop(affix::inject(arc_view))
        .get(index)
        .push(at("/ws").get(liveview))
}

#[handler]
fn index(res: &mut Response) -> Result<()> {
    let ws_addr = &env().ws_host;
    let liveview_js = dioxus_liveview::interpreter_glue(ws_addr);
    res.render(Text::Html(format!(
        r#"
            <!DOCTYPE html>
            <html lang=en class="h-full">
                <head>
                    <meta charset="utf-8">
                    <meta content="width=device-width, initial-scale=1" name="viewport">
                    <title>updown</title>
                    <script src="https://cdn.tailwindcss.com"></script>
                    <style>
                        .box-shadow-md {{ box-shadow: 0 6px var(--tw-shadow-color); }}
                        .hover\:box-shadow-xs:hover {{ box-shadow: 0 4px var(--tw-shadow-color); }}
                    </style>
                </head>
                <body class="h-full dark:bg-gray-950 bg-gray-50 dark:text-white text-gray-900">
                    <div id="main" class="h-full"></div>
                </body>
                {}
            </html>
        "#,
        liveview_js
    )));
    Ok(())
}

#[handler]
async fn liveview(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
) -> Result<(), StatusError> {
    let env_origin = &env().origin;
    let origin = &req.header::<String>(ORIGIN).unwrap_or_default();
    if env_origin != origin {
        return Err(StatusError::not_found());
    }
    let view = depot
        .obtain::<Arc<LiveViewPool>>()
        .expect("LiveViewPool was not found in the middleware")
        .clone();
    WebSocketUpgrade::new()
        .upgrade(req, res, |ws| async move {
            let _ = view
                .launch_with_props::<RootProps>(
                    dioxus_liveview::salvo_socket(ws),
                    Root,
                    RootProps {},
                )
                .await;
        })
        .await
}

#[derive(Props, PartialEq)]
struct RootProps {}

fn Root(cx: Scope<RootProps>) -> Element {
    let sites: &UseState<Vec<Site>> = use_state(cx, || vec![]);
    let onadd = move |event: FormEvent| {
        cx.spawn({
            to_owned![sites];
            async move {
                let url = match event.values.get("url") {
                    Some(values) => values.first().cloned().unwrap_or_default(),
                    None => String::with_capacity(0),
                };
                if url.is_empty() {
                    return;
                }
                let site = Site { url };
                sites.with_mut(|s| s.push(site));
            }
        })
    };
    cx.render(rsx! {
        div {
            class: "flex items-center justify-center h-full px-4 md:px-0",
            div {
                class: "flex flex-col max-w-md w-full gap-16",
                div {
                    h1 { class: "text-4xl text-center", "updown" }
                    h2 { class: "text-xl text-center", "your friendly neighborhood uptime monitor" }
                }
                AddSite {
                    onadd: onadd
                }
                div {
                    class: "flex flex-col gap-4",
                    sites.get().iter().map(|site| rsx! {
                        ShowSite {
                            key: "{site.url}",
                            site: site
                        }
                    })
                }
            }
        }
    })
}

#[derive(Props)]
struct AddSiteProps<'a> {
    onadd: EventHandler<'a, FormEvent>,
}

fn AddSite<'a>(cx: Scope<'a, AddSiteProps<'a>>) -> Element {
    cx.render(rsx! {
        form {
            onsubmit: move |event| cx.props.onadd.call(event),
            class: "flex flex-col gap-2 w-full",
            TextInput { name: "url", placeholder: "https://example.com" }
            Button { "Monitor a site" }
        }
    })
}

#[derive(Props, PartialEq)]
struct ShowSiteProps<'a> {
    site: &'a Site,
}

fn ShowSite<'a>(cx: Scope<'a, ShowSiteProps<'a>>) -> Element<'a> {
    let ShowSiteProps { site } = cx.props;
    let Site { url } = site;
    cx.render(rsx! {
        div {
            class: "border border-gray-200 dark:border-gray-800 dark:text-white p-2 rounded-md flex items-center justify-between",
            div { "{url}" }
            div {
                class: "flex items-center gap-x-1.5",
                div {
                    class: "flex-none rounded-full bg-emerald-500/20 p-1",
                    div {
                        class: "h-1.5 w-1.5 rounded-full bg-emerald-500"
                    }
                }
                p {
                    class: "text-xs leading-5 text-gray-500 dark:text-gray-400", "Online"
                }
            }
        }
    })
}

#[derive(Props)]
struct ButtonProps<'a> {
    #[props(optional)]
    onclick: Option<EventHandler<'a, MouseEvent>>,
    children: Element<'a>,
}

fn Button<'a>(cx: Scope<'a, ButtonProps<'a>>) -> Element {
    let ButtonProps { onclick, children } = cx.props;
    let onclick = move |event| {
        if let Some(click) = onclick {
            click.call(event);
        }
    };
    cx.render(rsx! {
        button {
            class: "bg-cyan-400 text-white px-2 py-3 rounded-3xl box-shadow-md shadow-cyan-600 hover:box-shadow-xs hover:top-0.5 active:shadow-none active:top-1 w-full relative",
            onclick: onclick,
            children
        }
    })
}

#[derive(Props)]
struct TextInputProps<'a> {
    #[props(optional)]
    placeholder: Option<&'a str>,
    name: &'a str,
}

fn TextInput<'a>(cx: Scope<'a, TextInputProps<'a>>) -> Element {
    let TextInputProps { name, placeholder } = cx.props;
    cx.render(rsx! {
        input {
            class: "rounded-lg px-2 py-3 border dark:border-cyan-500 outline-none text-black",
            r#type: "text",
            name: "{name}",
            placeholder: placeholder.unwrap_or_default()
        }
    })
}
