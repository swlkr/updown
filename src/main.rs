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
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, OnceLock},
};
use updown::{AppError, Database, Login, Site, User};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    ENV.set(Env::new()).unwrap();
    DB.set(Database::new(env().database_url.clone()).await)
        .unwrap();
    let args: Vec<String> = std::env::args().collect();
    let Some(arg) = args.get(1) else {
        db().migrate().await?;
        server().await?;
        return Ok(());
    };
    match arg.as_str() {
        "migrate" => {
            db().migrate().await?;
        }
        "rollback" => {
            db().rollback().await?;
        }
        "watch" => {
            watch().await?;
        }
        _ => todo!(),
    };
    Ok(())
}

async fn server() -> Result<()> {
    // hot_reload_init!();
    let addr: SocketAddr = env().host.parse()?;
    println!("Listening on {}", addr);
    Server::new(TcpListener::bind(addr)).serve(routes()).await;
    Ok(())
}

async fn watch() -> Result<()> {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));

    loop {
        interval.tick().await;
        tokio::spawn(async {
            _ = monitor().await;
        });
    }
}

async fn monitor() -> Result<()> {
    let sites = db().sites().await?;
    for site in sites {
        let response = response(&site).await?;
        db().upsert_response(response).await?;
    }
    Ok(())
}

async fn response<'a>(site: &'a Site) -> Result<updown::models::Response> {
    let status_code: i64 = reqwest::get(&site.url).await?.status().as_u16() as i64;
    let mut res = updown::models::Response::default();
    res.status_code = status_code;
    res.site_id = site.id;
    Ok(res)
}

#[derive(RustEmbed)]
#[folder = "static"]
struct Assets;

static ENV: OnceLock<Env> = OnceLock::new();
static DB: OnceLock<Database> = OnceLock::new();

#[derive(Debug, Default)]
struct Env {
    pub database_url: String,
    pub host: String,
    pub origin: String,
    pub ws_host: String,
    pub session_key: String,
}

impl Env {
    fn new() -> Self {
        Self::parse(Self::read())
    }

    fn read() -> String {
        std::fs::read_to_string(".env").unwrap_or_default()
    }

    fn parse(file: String) -> Self {
        let data = file
            .lines()
            .flat_map(|line| line.split("="))
            .collect::<Vec<_>>()
            .chunks_exact(2)
            .map(|x| (x[0], x[1]))
            .collect::<HashMap<_, _>>();
        Self {
            database_url: data
                .get("DATABASE_URL")
                .expect("DATABASE_URL is missing")
                .to_string(),
            host: data.get("HOST").expect("HOST is missing").to_string(),
            origin: data.get("ORIGIN").expect("ORIGIN is missing").to_string(),
            ws_host: data.get("WS_HOST").expect("WS_HOST is missing").to_string(),
            session_key: data
                .get("SESSION_KEY")
                .expect("SESSION_KEY is missing")
                .to_string(),
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
        .same_site_policy(SameSite::Lax)
        .session_ttl(Some(std::time::Duration::from_secs(604_800)))
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

#[cfg(debug_assertions)]
const TAILWIND_CSS: &'static str = r#"<script src="https://cdn.tailwindcss.com"></script>"#;
#[cfg(not(debug_assertions))]
const TAILWIND_CSS: &'static str = r#"<link href="./tailwind.css" rel="stylesheet" />"#;
#[cfg(debug_assertions)]
const RETRY_MS: u16 = 1_000;
#[cfg(not(debug_assertions))]
const RETRY_MS: u16 = 45_000;

#[handler]
async fn index(res: &mut Response) -> Result<()> {
    let ws_addr = &env().ws_host;
    res.render(Text::Html(format!(
        r#"
            <!DOCTYPE html>
            <html lang=en class="h-full">
                <head>
                    <meta charset="utf-8">
                    <meta content="width=device-width, initial-scale=1" name="viewport">
                    <meta name="ws-addr" content="{ws_addr}"">
                    <meta name="retry-ms" content="{RETRY_MS}">
                    <title>updown</title>
                    {TAILWIND_CSS}
                    <style>
                        .box-shadow-md {{ box-shadow: 0 6px var(--tw-shadow-color); }}
                        .hover\:box-shadow-xs:hover {{ box-shadow: 0 4px var(--tw-shadow-color); }}
                    </style>
                    <script defer src="./main.js"></script>
                </head>
                <body class="h-full dark:bg-gray-950 bg-gray-50 dark:text-white text-gray-900">
                    <div id="main" class="h-full"></div>
                </body>
            </html>
        "#
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
    let (sites, login_count) = if let Some(user) = &current_user {
        let sites = db().sites_by_user_id(user.id).await.unwrap_or(vec![]);
        let login_count = db().login_count(user.id).await.unwrap_or(0);
        (sites, login_count)
    } else {
        (vec![], 0)
    };
    WebSocketUpgrade::new()
        .upgrade(req, res, move |ws| async move {
            let _ = view
                .launch_with_props::<RootProps>(
                    dioxus_liveview::salvo_socket(ws),
                    Root,
                    RootProps {
                        current_user,
                        login_count,
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
    login_count: i32,
}

fn Root(cx: Scope<RootProps>) -> Element {
    let RootProps {
        current_user,
        sites,
        login_count,
    } = cx.props;
    use_shared_state_provider(cx, || RootProps {
        current_user: cx.props.current_user.clone(),
        sites: sites.clone(),
        login_count: login_count.clone(),
    });
    let initial_view = match (current_user, login_count) {
        (Some(_), 1) => View::Account,
        (Some(_), _) => View::Monitors,
        (None, _) => View::default(),
    };
    let view = use_state(cx, || initial_view);
    let onnav = move |new_view| {
        to_owned![view];
        view.set(new_view);
    };
    let add_site_sheet_shown = use_state(cx, || false);
    let show_add_site_sheet = move |_| {
        add_site_sheet_shown.set(!add_site_sheet_shown.get());
    };
    let user_id = match current_user {
        Some(u) => u.id,
        None => 0,
    };
    let sites = use_state(cx, || sites.clone());
    let onadd = move |event: FormEvent| {
        cx.spawn({
            to_owned![sites, user_id, add_site_sheet_shown];
            if user_id == 0 {
                return;
            }
            let url = match event.values.get("url") {
                Some(values) => values.first().cloned().unwrap_or_default(),
                None => String::with_capacity(0),
            };
            if url.is_empty() {
                return;
            }
            async move {
                let mut site = Site::default();
                site.user_id = user_id;
                site.url = url;
                match db().insert_site(site).await {
                    Ok(s) => {
                        sites.with_mut(|sites| sites.insert(0, s));
                        add_site_sheet_shown.set(false);
                    }
                    Err(_) => {}
                }
            }
        })
    };
    cx.render(rsx! {
        div {
            class: "flex flex-col justify-center md:items-center pt-4 md:pt-16 lg:pt-32 px-4 md:px-0 max-w-md mx-auto gap-4 md:gap-16 md:mb-0 pb-32 overflow-auto",
            Header {}
            match view.get() {
                View::Index => rsx! {
                    Index {}
                },
                View::Monitors => rsx! {
                    Monitors {
                        sites: sites.get()
                    }
                },
                View::Login => rsx! {
                    NewLogin {}
                },
                View::Account => rsx! {
                    Account { onnav: onnav, current_user: current_user }
                }
            }
        }
        Nav { onclick: onnav, active_view: view.get() }
        if current_user.is_some() {
            rsx! {
                Fab {
                    onclick: show_add_site_sheet,
                    div { class: "text-2xl", "+" }
                }
                Sheet {
                    shown: *add_site_sheet_shown.get(),
                    onclose: move |_| {
                        to_owned![add_site_sheet_shown];
                        add_site_sheet_shown.set(false);
                    }
                    div {
                        AddSite { onadd: onadd }
                    }
                }
            }
        }
    })
}

#[inline_props]
fn Sheet<'a>(
    cx: Scope,
    shown: bool,
    onclose: EventHandler<'a>,
    children: Element<'a>,
) -> Element<'a> {
    let translate_y = match shown {
        true => "",
        false => "translate-y-full",
    };
    return cx.render(
        rsx! {
            div {
                class: "transition ease-out overflow-y-auto {translate_y} min-h-[80%] left-0 right-0 bottom-0 lg:max-w-3xl lg:mx-auto fixed p-6 rounded-md bg-gray-50 dark:bg-gray-900 z-30",
                div {
                    class: "flex justify-end items-end mb-6",
                    CircleButton {
                        onclick: move |_| onclose.call(()),
                        div { class: "text-2xl mb-1", "x" }
                    }
                }
                children
            }
        }
    );
}

#[inline_props]
fn CircleButton<'a>(cx: Scope, onclick: EventHandler<'a>, children: Element<'a>) -> Element<'a> {
    cx.render(rsx! {
        button {
            class: "rounded-full bg-gray-800 w-12 h-12 flex justify-center items-center",
            onclick: move |_| onclick.call(()),
            children
        }
    })
}

fn NewLogin(cx: Scope) -> Element {
    cx.render(rsx! {
        div {
            class: "flex flex-col gap-2 w-full",
            TextInput { placeholder: "Your login code goes here", name: "login-code" }
            Button { id: "login-btn", "Login" }
        }
    })
}

fn Index(cx: Scope) -> Element {
    cx.render(rsx! {
        AddSite { id: "signup-btn" }
    })
}

#[inline_props]
fn Monitors<'a>(cx: Scope, sites: &'a Vec<Site>) -> Element {
    cx.render(rsx! {
        div {
            class: "flex flex-col gap-4",
            sites.iter().map(|site| rsx! {
                ShowSite {
                    key: "{site.id}",
                    site: site
                }
            })
        }
    })
}

fn Header(cx: Scope) -> Element {
    cx.render(rsx! {
        div {
            class: "flex md:flex-col md:gap-2",
            h1 { class: "text-xl md:text-4xl md:text-center text-left", "updown" }
            h2 { class: "text-xl text-center hidden md:block", "your friendly neighborhood uptime monitor" }
        }
    })
}

#[derive(Default, Clone, PartialEq)]
enum View {
    #[default]
    Index,
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
            class: "fixed lg lg:top-0 lg:bottom-auto bottom-0 w-full py-6 dark:bg-gray-900",
            ul {
                class: "flex lg:justify-center lg:gap-4 justify-around",
                if logged_in {
                    rsx! {
                        NavLink { active: **active_view == View::Monitors, onclick: move |_| onclick.call(View::Monitors), "Sites" }
                        NavLink { active: **active_view == View::Account, onclick: move |_| onclick.call(View::Account), "Account" }
                        NavLink { id: "logout-btn", "Logout" }
                    }
                } else {
                    rsx! {
                        NavLink { active: **active_view == View::Index, onclick: move |_| onclick.call(View::Index), "Home" }
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
    active: Option<bool>,
    onclick: Option<EventHandler<'a, ()>>,
    id: Option<&'a str>,
    children: Element<'a>,
) -> Element {
    let active_class = match active {
        Some(true) => "text-cyan-400",
        Some(false) | None => "",
    };
    let onclick = move |_| {
        if let Some(click) = onclick {
            click.call(())
        }
    };
    let id = id.unwrap_or_default();
    cx.render(rsx! {
        li {
            class: "cursor-pointer group transition duration-300", onclick: onclick,
            a { id: id, class: "{active_class}", children }
            div { class: "max-w-0 group-hover:max-w-full transition-all duration-300 h-1 bg-cyan-400" }
        }
    })
}

#[inline_props]
fn AddSite<'a>(
    cx: Scope,
    id: Option<&'a str>,
    onadd: Option<EventHandler<'a, FormEvent>>,
) -> Element {
    let id = id.unwrap_or_default();
    let onsubmit = move |event| {
        if let Some(onadd) = onadd {
            onadd.call(event)
        }
    };
    cx.render(rsx! {
        form {
            onsubmit: onsubmit,
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
            class: "px-4 py-3 w-full bg-cyan-400 text-white rounded-3xl box-shadow-md shadow-cyan-600 hover:box-shadow-xs hover:top-0.5 active:shadow-none active:top-1 relative",
            onclick: onclick,
            children
        }
    })
}

fn Fab<'a>(cx: Scope<'a, ButtonProps<'a>>) -> Element {
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
        div {
            class: "absolute bottom-24 right-4 z-20",
            button {
                id: id,
                class: "h-12 w-12 rounded-full bg-cyan-400 text-white box-shadow-md shadow-cyan-600 hover:box-shadow-xs hover:top-0.5 active:shadow-none active:top-1 relative",
                onclick: onclick,
                children
            }
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
            class: "rounded-lg px-2 py-3 border dark:border-gray-700 dark:text-white dark:bg-gray-800 outline-none text-black",
            r#type: "text",
            name: "{name}",
            placeholder: placeholder.unwrap_or_default()
        }
    })
}

#[inline_props]
fn Account<'a>(
    cx: Scope,
    current_user: &'a Option<User>,
    onnav: EventHandler<'a, View>,
) -> Element {
    let login_code = match current_user {
        Some(u) => u.login_code.clone(),
        None => "".to_string(),
    };
    cx.render(rsx! {
        div {
            class: "grid place-content-center gap-4",
            LoginCodeAlert { login_code: login_code }
            Button { onclick: move |_| onnav.call(View::Monitors), "View your sites" }
        }
    })
}

#[inline_props]
fn LoginCodeAlert(cx: Scope, login_code: String) -> Element {
    let blur_class = use_state(cx, || "blur-sm");
    let onclick = move |_| {
        to_owned![blur_class];
        if blur_class == "blur-sm" {
            blur_class.set("");
        } else {
            blur_class.set("blur-sm");
        }
    };
    cx.render(rsx! {
        div {
            class: "bg-blue-50 p-4 rounded-md text-blue-500 flex flex-col gap-1",
            p { "This is the only identifier you need to use updown." }
            p { "No email, no username. Just simplicity." }
            p { "Click to show your login code." }
            div { onclick: onclick, class: "cursor-pointer font-bold text-xl {blur_class}", "{login_code}" }
        }
    })
}
