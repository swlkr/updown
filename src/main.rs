#![allow(non_snake_case)]

use anyhow::Result;
use dioxus::prelude::*;
use dioxus_liveview::LiveViewPool;
use nanoid::nanoid;
use salvo::{
    affix, handler,
    http::cookie::SameSite,
    hyper::header::ORIGIN,
    prelude::{StatusError, TcpListener},
    session::{CookieStore, SessionDepotExt, SessionHandler},
    writer::{Json, Text},
    ws::WebSocketUpgrade,
    Depot, Request, Response, Router, Server,
};
use serde::{Deserialize, Serialize};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    FromRow, SqlitePool,
};
use std::{
    net::SocketAddr,
    sync::{Arc, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
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
    pub session_key: String,
}

impl Env {
    fn new() -> Self {
        let database_url = std::env::var("DATABASE_URL").unwrap();
        let host = std::env::var("HOST").unwrap();
        let origin = std::env::var("ORIGIN").unwrap();
        let ws_host = std::env::var("WS_HOST").unwrap();
        let app_env = std::env::var("APP_ENV").unwrap();
        let session_key = std::env::var("SESSION_KEY").unwrap();
        Self {
            database_url,
            host,
            origin,
            ws_host,
            app_env,
            session_key,
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

    pub async fn user_by_id(&self, id: i64) -> Result<User, sqlx::Error> {
        sqlx::query_as!(User, "select * from users where id = ?", id)
            .fetch_one(&self.connection)
            .await
    }

    pub async fn user_by_login_code(&self, login_code: String) -> Result<User, sqlx::Error> {
        sqlx::query_as!(
            User,
            "select id as 'id!', login_code, created_at, updated_at from users where login_code = ? limit 1",
            login_code
        )
        .fetch_one(&self.connection)
        .await
    }

    // pub async fn insert_login(&self, user: User) -> Result<Login, sqlx::Error> {
    //     sqlx::query_as!(Login, "insert into logins (user_id, created_at) values (?, ?) returning *").fetch_one(&self.connection).await
    // }

    // async fn hide_login_code_flash(&self, user: User) -> Result<User, sqlx::Error> {
    //     sqlx::query_as!(
    //         User,
    //         "update users set login_code_hidden = ? where id = ?",
    //         true,
    //         user.id
    //     )
    //     .fetch_one(&self.connection)
    //     .await
    // }
}

#[derive(Serialize, Deserialize, Default, Clone, PartialEq, FromRow, Debug)]
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
    let session_key = &env().session_key;
    let session_handler = SessionHandler::builder(CookieStore::new(), &session_key.as_bytes())
        .cookie_name("id")
        .same_site_policy(SameSite::Strict)
        .build()
        .unwrap();
    let view = LiveViewPool::new();
    let arc_view = Arc::new(view);
    Router::new()
        .hoop(session_handler)
        .hoop(set_current_user_handler)
        .hoop(affix::inject(arc_view))
        .get(index)
        .push(at("/login").post(login))
        .push(at("/logout").post(logout))
        .push(at("/ws").get(liveview))
}

#[derive(Serialize, Deserialize)]
struct LoginParams {
    login_code: String,
}

#[handler]
async fn login(depot: &mut Depot, req: &mut Request) -> Result<Json<User>> {
    let LoginParams { login_code } = req.parse_json::<LoginParams>().await?;
    let user = db().user_by_login_code(login_code.clone()).await?;
    if let Some(session) = depot.session_mut() {
        session
            .insert("user_id", user.id)
            .expect("could not set user id in session");
    }
    Ok(Json(User::default()))
}

#[handler]
async fn logout(depot: &mut Depot) -> Result<Json<User>> {
    if let Some(session) = depot.session_mut() {
        if let Some(user_id) = session.get::<i64>("user_id") {
            if let Some(_) = db().user_by_id(user_id).await.ok() {
                session.remove("user_id");
            }
        }
    }
    Ok(Json(User::default()))
}

#[handler]
async fn set_current_user_handler(depot: &mut Depot) {
    let maybe_id: Option<i64> = depot.session().unwrap().get("user_id");
    if let Some(id) = maybe_id {
        if let Ok(user) = db().user_by_id(id).await {
            depot.inject(user);
        }
    }
}

#[handler]
async fn index(res: &mut Response) -> Result<()> {
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
                    <script>
                        async function login(login_code) {{
                            const response = await fetch("/login", {{
                                method: "POST",
                                headers: {{
                                    "Accept": "application/json",
                                    "Content-Type": "application/json"
                                }},
                                body: JSON.stringify({{ login_code: login_code }})
                            }});
                            const _ = await response.json();
                            window.location.reload();
                        }}

                        async function logout() {{
                            const response = await fetch("/logout", {{
                                method: "POST",
                                headers: {{
                                    "Content-Type": "application/json"
                                }}
                            }});
                            const _ = await response.json();
                            window.location.reload();
                        }}

                        document.addEventListener("click", (event) => {{
                            if(event.target.id === "login-btn") {{
                                const login_code = document.querySelector('input[name="login-code"]').value;
                                login(login_code).then(x => x);
                            }}
                            if(event.target.id === "logout-btn") {{
                                logout().then(x => x);
                            }}
                        }});
                    </script>
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
    let current_user = depot.obtain::<User>().cloned();
    WebSocketUpgrade::new()
        .upgrade(req, res, |ws| async move {
            let _ = view
                .launch_with_props::<RootProps>(
                    dioxus_liveview::salvo_socket(ws),
                    Root,
                    RootProps { current_user },
                )
                .await;
        })
        .await
}

#[derive(Props, PartialEq)]
struct RootProps {
    current_user: Option<User>,
}

fn Root(cx: Scope<RootProps>) -> Element {
    let RootProps { current_user } = cx.props;
    use_shared_state_provider(cx, || RootProps {
        current_user: cx.props.current_user.clone(),
    });
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
    let initial_view = match current_user {
        Some(_) => View::Monitors,
        None => View::default(),
    };
    let view = use_state(cx, || initial_view);
    let onnav = move |new_view| {
        to_owned![view];
        view.set(new_view);
    };
    cx.render(rsx! {
        div {
            class: "flex flex-col justify-center items-center pt-16 px-4 md:px-0 max-w-md mx-auto gap-16",
            Header {}
            match view.get() {
                View::Lander => rsx! {
                    Lander { onclick: move |_| { to_owned![view]; view.set(View::NewAccount); } }
                },
                View::NewAccount => rsx! {
                    NewAccount {}
                },
                View::Monitors => rsx! {
                    Monitors {
                        onadd: onadd,
                        sites: sites.get(),
                    }
                },
                View::Login => rsx! {
                    Login {}
                },
                View::Account => rsx! {
                    Account {}
                }
            }
        }
        Nav { onclick: onnav }
    })
}

fn Login(cx: Scope) -> Element {
    cx.render(rsx! {
        div {
            class: "flex flex-col gap-2",
            TextInput { name: "login-code" }
            Button { id: "login-btn", "Login" }
        }
    })
}

#[inline_props]
fn Monitors<'a>(cx: Scope, onadd: EventHandler<'a, FormEvent>, sites: &'a Vec<Site>) -> Element {
    cx.render(rsx! {
        AddSite { onadd: move |e| onadd.call(e) }
        div {
            class: "flex flex-col gap-4",
            sites.iter().map(|site| rsx! {
                ShowSite {
                    key: "{site.url}",
                    site: site
                }
            })
        }
    })
}

fn Header(cx: Scope) -> Element {
    cx.render(rsx! {
        div {
            h1 { class: "text-4xl text-center", "updown" }
            h2 { class: "text-xl text-center", "your friendly neighborhood uptime monitor" }
        }
    })
}

#[inline_props]
fn Lander<'a>(cx: Scope, onclick: EventHandler<'a, MouseEvent>) -> Element {
    cx.render(rsx! {
        Button { onclick: move |e| onclick.call(e), "Sign up and start monitoring" }
    })
}

fn NewAccount(cx: Scope) -> Element {
    let login_code = use_state(cx, || None);
    let onclick = move |_| {
        cx.spawn({
            to_owned![login_code];
            async move {
                if let Ok(user) = db().insert_user().await {
                    login_code.set(Some(user.login_code.clone()));
                }
            }
        })
    };
    cx.render(rsx! {
        div {
            h1 { class: "text-2xl text-center", "Get a login code", }
            match login_code.get() {
                Some(c) => {
                    rsx! {
                        div {
                            class: "flex flex-col gap-6",
                            p { "Congrats! Here's your login code" }
                            h1 { class: "text-2xl font-bold", "{c}" }
                            LoginCodeAlert { login_code: String::default() }
                            input { r#type: "hidden", name: "login-code", value: "{c}" }
                            Button { id: "login-btn", "Add a site to monitor" }
                        }
                    }
                },
                None => {
                    rsx! {
                        Button {
                            onclick: onclick,
                            "Generate login code"
                        }
                    }
                }
            }
        }
    })
}

#[derive(Default, Clone)]
enum View {
    #[default]
    Lander,
    NewAccount,
    Monitors,
    Account,
    Login,
}

#[derive(Props)]
struct NavProps<'a> {
    onclick: EventHandler<'a, View>,
}

fn Nav<'a>(cx: Scope<'a, NavProps<'a>>) -> Element {
    let ss = use_shared_state::<RootProps>(cx).unwrap();
    let logged_in = ss.read().current_user.is_some();
    let default_view = match logged_in {
        true => View::Monitors,
        false => View::Lander,
    };
    cx.render(rsx! {
        nav {
            class: "fixed bottom-0 w-full py-8",
            ul {
                class: "flex justify-around",
                li { class: "cursor-pointer", onclick: move |_| { cx.props.onclick.call(default_view.to_owned()) }, "Home" }
                if logged_in {
                    rsx! {
                        li { class: "cursor-pointer", onclick: move |_| cx.props.onclick.call(View::Account), "Account"  }
                    }
                } else {
                    rsx! {
                        li { class: "cursor-pointer", onclick: move |_| cx.props.onclick.call(View::Login), "Login" }
                        li { class: "cursor-pointer", "Sign up" }
                    }
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
    id: Option<&'a str>,
    #[props(optional)]
    onclick: Option<EventHandler<'a, MouseEvent>>,
    children: Element<'a>,
}

fn Button<'a>(cx: Scope<'a, ButtonProps<'a>>) -> Element {
    let ButtonProps {
        id,
        onclick,
        children,
    } = cx.props;
    let onclick = move |event| {
        if let Some(click) = onclick {
            click.call(event);
        }
    };
    let id = id.unwrap_or_default();
    cx.render(rsx! {
        button {
            id: id,
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

fn current_user(cx: Scope) -> User {
    let ss = use_shared_state::<RootProps>(cx).unwrap();
    ss.read().current_user.clone().unwrap()
}

fn Account(cx: Scope) -> Element {
    let User { login_code, .. } = current_user(cx);
    cx.render(rsx! {
        div {
            class: "grid place-content-center gap-4",
            LoginCodeAlert { login_code: login_code }
            Button { id: "logout-btn", "Logout" }
        }
    })
}

#[inline_props]
fn LoginCodeAlert(cx: Scope, login_code: String) -> Element {
    cx.render(rsx! {
        div {
            class: "bg-blue-50 p-4 rounded-md text-blue-500 flex flex-col gap-1",
            p { "This is the only identifier you need to use updown." }
            p { "No email, no username. Just simplicity." }
            div { class: "font-bold text-2xl", "{login_code}" }
        }
    })
}
