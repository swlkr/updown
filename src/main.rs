#![allow(non_snake_case)]

use anyhow::Result;
use dioxus::prelude::*;
use dioxus_liveview::LiveViewPool;
use rust_embed::RustEmbed;
use salvo::{
    affix, handler,
    http::cookie::SameSite,
    hyper::header::ORIGIN,
    prelude::{StatusCode, StatusError, TcpListener},
    serve_static::static_embed,
    session::{CookieStore, SessionDepotExt, SessionHandler},
    writer::{Json, Text},
    ws::WebSocketUpgrade,
    Depot, Request, Response, Router, Server,
};
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    sync::{Arc, OnceLock},
};
use updown::{AppError, Database, Login, Site, User};

#[derive(RustEmbed)]
#[folder = "static"]
struct Assets;

#[tokio::main]
async fn main() -> Result<()> {
    // hot_reload_init!();
    ENV.set(Env::new()).unwrap();
    DB.set(Database::new(env().database_url.clone()).await)
        .unwrap();
    tracing_subscriber::fmt().init();
    let addr: SocketAddr = env().host.parse()?;
    println!("Listening on {}", addr);
    Server::new(TcpListener::bind(addr)).serve(routes()).await;
    Ok(())
}

static ENV: OnceLock<Env> = OnceLock::new();
static DB: OnceLock<Database> = OnceLock::new();

#[derive(Debug)]
struct Env {
    pub database_url: String,
    pub host: String,
    pub origin: String,
    pub ws_host: String,
    pub session_key: String,
}

impl Env {
    fn new() -> Self {
        let database_url = std::env::var("DATABASE_URL").unwrap();
        let host = std::env::var("HOST").unwrap();
        let origin = std::env::var("ORIGIN").unwrap();
        let ws_host = std::env::var("WS_HOST").unwrap();
        let session_key = std::env::var("SESSION_KEY").unwrap();
        Self {
            database_url,
            host,
            origin,
            ws_host,
            session_key,
        }
    }
}

fn env() -> &'static Env {
    ENV.get().expect("env is not initialized")
}

pub fn db() -> &'static Database {
    DB.get().expect("db is not initialized")
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
        .push(
            Router::new()
                .hoop(session_handler)
                .hoop(set_current_user_handler)
                .hoop(affix::inject(arc_view))
                .get(index)
                .push(at("/login").post(login))
                .push(at("/signup").post(signup))
                .push(at("/logout").post(logout))
                .push(at("/ws").get(liveview)),
        )
        .push(at("<**path>").get(static_embed::<Assets>()))
}

#[derive(Serialize, Deserialize)]
struct LoginParams {
    login_code: String,
}

#[handler]
async fn login(depot: &mut Depot, req: &mut Request, res: &mut Response) -> Result<()> {
    let LoginParams { login_code } = req.parse_json::<LoginParams>().await?;
    let user = db().user_by_login_code(login_code).await?;
    let session = depot.session_mut().ok_or(AppError::Login)?;
    _ = session.insert("user_id", user.id)?;
    let new_login: Login = Database::new_login(user.id);
    if let Ok(_) = db().insert_login(new_login).await {
        res.set_status_code(StatusCode::OK);
        res.render(Json(Login::default()));
    } else {
        res.set_status_code(StatusCode::UNAUTHORIZED);
        res.render(Json(AppError::Login));
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct SignupParams {
    url: String,
}

#[handler]
async fn signup(depot: &mut Depot, req: &mut Request, res: &mut Response) -> Result<()> {
    let SignupParams { url } = req.parse_json::<SignupParams>().await?;
    if url.is_empty() {
        res.set_status_code(StatusCode::UNPROCESSABLE_ENTITY);
        res.render(Json(AppError::UrlEmpty));
        return Ok(());
    }
    let user = db().insert_user().await?;
    let session = depot.session_mut().ok_or(AppError::Login)?;
    session
        .insert("user_id", user.id)
        .expect("could not set user id in session");
    let mut login_row = Login::default();
    login_row.user_id = user.id;
    _ = db().insert_login(login_row).await?;
    let mut site = Site::default();
    site.user_id = user.id;
    site.url = url;
    if let Ok(_) = db().insert_site(site).await {
        res.set_status_code(StatusCode::OK);
        res.render(Json(AppError::Login));
    } else {
        res.set_status_code(StatusCode::UNAUTHORIZED);
        res.render(Json(AppError::Login));
    }
    Ok(())
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
                    <script defer src="./main.js"></script>
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
    let current_user = depot.obtain::<User>().cloned();
    let sites = match &current_user {
        Some(u) => db().sites_by_user_id(u.id).await.unwrap_or(vec![]),
        None => vec![],
    };
    WebSocketUpgrade::new()
        .upgrade(req, res, |ws| async move {
            let _ = view
                .launch_with_props::<RootProps>(
                    dioxus_liveview::salvo_socket(ws),
                    Root,
                    RootProps {
                        current_user,
                        sites,
                    },
                )
                .await;
        })
        .await
}

#[derive(Props, PartialEq)]
struct RootProps {
    current_user: Option<User>,
    sites: Vec<Site>,
}

fn Root(cx: Scope<RootProps>) -> Element {
    let RootProps {
        current_user,
        sites,
    } = cx.props;
    use_shared_state_provider(cx, || RootProps {
        current_user: cx.props.current_user.clone(),
        sites: sites.clone(),
    });
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
            class: "flex flex-col justify-center items-center pt-16 lg:pt-32 px-4 md:px-0 max-w-md mx-auto gap-16",
            Header {}
            match view.get() {
                View::Monitors => rsx! {
                    Monitors {}
                },
                View::Login => rsx! {
                    NewLogin {}
                },
                View::Account => rsx! {
                    Account {}
                }
            }
        }
        Nav { onclick: onnav, active_view: view.get() }
    })
}

fn NewLogin(cx: Scope) -> Element {
    cx.render(rsx! {
        div {
            class: "flex flex-col gap-2",
            TextInput { name: "login-code" }
            Button { id: "login-btn", "Login" }
        }
    })
}

fn Monitors(cx: Scope) -> Element {
    let shared_state = use_shared_state::<RootProps>(cx).unwrap();
    let current_user = shared_state.read().current_user.clone();
    let sites = shared_state.read().sites.clone();
    let login_code = match current_user {
        Some(ref u) => Some(u.login_code.clone()),
        None => None,
    };
    let user_id = match current_user {
        Some(u) => u.id,
        None => 0,
    };
    let sites: &UseState<Vec<Site>> = use_state(cx, || sites.clone());
    let onadd = move |event: FormEvent| {
        cx.spawn({
            to_owned![sites, user_id];
            if user_id == 0 {
                return;
            }
            async move {
                let url = match event.values.get("url") {
                    Some(values) => values.first().cloned().unwrap_or_default(),
                    None => String::with_capacity(0),
                };
                if url.is_empty() {
                    return;
                }
                let mut site = Site::default();
                site.user_id = user_id;
                site.url = url;
                match db().insert_site(site).await {
                    Ok(s) => {
                        sites.with_mut(|sites| sites.insert(0, s));
                    }
                    Err(_) => {}
                }
            }
        })
    };
    cx.render(rsx! {
        if login_code.is_some() {
            rsx! {
                LoginCodeAlert { login_code: login_code.unwrap() }
            }
        }
        AddSite { onadd: onadd }
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

#[derive(Default, Clone, PartialEq)]
enum View {
    #[default]
    Monitors,
    Account,
    Login,
}

#[inline_props]
fn Nav<'a>(cx: Scope, onclick: EventHandler<'a, View>, active_view: &'a View) -> Element {
    let ss = use_shared_state::<RootProps>(cx).unwrap();
    let logged_in = ss.read().current_user.is_some();
    cx.render(rsx! {
        nav {
            class: "fixed lg lg:top-0 lg:bottom-auto bottom-0 w-full py-8",
            ul {
                class: "flex lg:justify-center lg:gap-4 justify-around",
                NavLink { active: **active_view == View::Monitors, onclick: move |_| onclick.call(View::Monitors), "Home" }
                if logged_in {
                    rsx! {
                        NavLink { active: **active_view == View::Account, onclick: move |_| onclick.call(View::Account), "Account" }
                    }
                } else {
                    rsx! {
                        NavLink { active: **active_view == View::Login, onclick: move |_| onclick.call(View::Login), "Login" }
                    }
                }
            }
        }
    })
}

#[inline_props]
fn NavLink<'a>(
    cx: Scope,
    active: bool,
    onclick: EventHandler<'a, ()>,
    children: Element<'a>,
) -> Element {
    let active_class = match active {
        true => "text-cyan-400",
        false => "",
    };
    cx.render(rsx! {
        li {
            class: "cursor-pointer group transition duration-300",
            a { class: "{active_class}", onclick: move |_| onclick.call(()), children }
            div { class: "max-w-0 group-hover:max-w-full transition-all duration-300 h-1 bg-cyan-400" }
        }
    })
}

#[inline_props]
fn AddSite<'a>(cx: Scope, onadd: EventHandler<'a, FormEvent>) -> Element {
    let shared_state = use_shared_state::<RootProps>(cx).unwrap();
    let current_user = shared_state.read().current_user.clone();
    let id = match current_user {
        Some(_) => "",
        None => "signup-btn",
    };
    cx.render(rsx! {
        form {
            onsubmit: move |event| onadd.call(event),
            class: "flex flex-col gap-2 w-full",
            TextInput { name: "url", placeholder: "https://example.com" }
            Button { id: "{id}", "Monitor a site" }
        }
    })
}

#[derive(Props, PartialEq)]
struct ShowSiteProps<'a> {
    site: &'a Site,
}

fn ShowSite<'a>(cx: Scope<'a, ShowSiteProps<'a>>) -> Element<'a> {
    let ShowSiteProps { site } = cx.props;
    let Site { url, id, .. } = site;
    // let response = use_state(cx, || Response::default());
    let response_future = use_future(cx, (), |_| {
        to_owned![id];
        async move { db().latest_response_by_site(id).await }
    });
    let status = match response_future.value() {
        Some(Ok(response)) => {
            if response.status_code >= 200 && response.status_code < 300 {
                "Online"
            } else {
                "Offline"
            }
        }
        Some(Err(_)) => "Unknown",
        None => "Loading",
    };
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
                    class: "text-xs leading-5 text-gray-500 dark:text-gray-400", "{status}"
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

fn current_user(cx: Scope) -> Option<User> {
    if let Some(ss) = use_shared_state::<RootProps>(cx) {
        ss.read().current_user.clone()
    } else {
        None
    }
}

fn Account(cx: Scope) -> Element {
    // current user is required here
    // this should panic if this component
    // is somehow accessed without user
    let login_code = current_user(cx).unwrap().login_code.clone();
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
