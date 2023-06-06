#![allow(non_snake_case)]

use anyhow::Result;
use dioxus::prelude::*;
use dioxus_hot_reload::Config;
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
    hot_reload_init!(Config::new()
        .with_logging(true)
        .with_rebuild_command("cargo run"));
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

#[derive(Clone, PartialEq)]
struct ServerCx {
    liveview_js: String,
    ws_addr: String,
}

#[handler]
fn index(res: &mut Response) -> Result<()> {
    let ws_addr = "ws://localhost:9001/ws".to_string();
    let liveview_js = dioxus_liveview::interpreter_glue(&ws_addr);
    res.render(Text::Html(format!(
        r#"
            <!DOCTYPE html>
            <html>
                <head> 
                    <title>updown</title>
                    <script src="https://cdn.tailwindcss.com"></script>
                </head>
                <body> 
                    <div id="main"></div> 
                </body>
                {}
            </html>
        "#,
        liveview_js
    )));
    Ok(())
}

#[derive(Props, PartialEq)]
struct RootProps {}

fn Root(cx: Scope<RootProps>) -> Element {
    let count = use_state(cx, || 0);
    let inc = move |_| {
        to_owned![count];
        count += 1;
    };
    let dec = move |_| {
        to_owned![count];
        count -= 1;
    };
    cx.render(rsx! {
        div {
            class: "flex flex-col gap-4",
            h1 { "count: {count}" }
            div {
                class: "flex gap-2",
                button { class: "bg-sky-500 w-12 rounded-lg", onclick: inc, "inc"  }
                button { class: "bg-sky-500 w-12 rounded-lg", onclick: dec, "dec"  }

            }
        }
    })
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
