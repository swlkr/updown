#![allow(non_snake_case)]

use anyhow::Result;
use dioxus::prelude::*;
use dioxus_liveview::LiveViewPool;
use salvo::{
    affix, handler,
    hyper::header::ORIGIN,
    prelude::{StatusError, TcpListener},
    writer::Text,
    ws::WebSocketUpgrade,
    Depot, Request, Response, Router, Server,
};
use std::{net::SocketAddr, sync::Arc};

#[tokio::main]
async fn main() -> Result<()> {
    hot_reload_init!();
    let addr: SocketAddr = "127.0.0.1:9001"
        .parse()
        .expect("Expected a string in the form of <ip address>:<port>");

    println!("Listening on {}", addr);

    Server::new(TcpListener::bind(addr)).serve(routes()).await;

    return Ok(());
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
    let ws_addr = "ws://localhost:9001/ws".to_string();
    let liveview_js = dioxus_liveview::interpreter_glue(&ws_addr);
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
    let addr = "http://localhost:9001";
    let origin = req.header::<String>(ORIGIN).unwrap_or_default();
    if addr != origin {
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

#[derive(Clone, PartialEq)]
struct Site {
    url: String,
}

#[derive(Props, PartialEq)]
struct RootProps {}

fn Root(cx: Scope<RootProps>) -> Element {
    let sites: &UseState<Vec<Site>> = use_state(cx, || vec![]);
    let onsubmit = move |event: FormEvent| {
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
                class: "flex flex-col max-w-sm w-full gap-16",
                div {
                    h1 { class: "text-4xl text-center", "updown" }
                    h2 { class: "text-xl text-center", "your friendly neighborhood uptime monitor" }
                }
                form {
                    onsubmit: onsubmit,
                    class: "flex flex-col gap-2 w-full",
                    TextInput { name: "url", placeholder: "https://example.com" }
                    Button { "Monitor a site" }
                }
                div {
                    class: "flex flex-col gap-4",
                    sites.get().iter().map(|site| rsx! {
                        SiteComponent {
                            key: "{site.url}",
                            site: site
                        }
                    })
                }
            }
        }
    })
}

#[derive(Props, PartialEq)]
struct SiteComponentProps<'a> {
    site: &'a Site,
}

fn SiteComponent<'a>(cx: Scope<'a, SiteComponentProps<'a>>) -> Element<'a> {
    let SiteComponentProps { site } = cx.props;
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
