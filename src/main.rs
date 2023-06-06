#![allow(non_snake_case)]

use anyhow::Result;
use dioxus::prelude::*;
use dioxus_hot_reload::Config;
use dioxus_liveview::LiveViewPool;
use dioxus_ssr::render_lazy;
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
    hot_reload_init!(Config::new().with_rebuild_command("cargo run"));
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

#[derive(Props, PartialEq)]
struct IndexProps {
    sx: ServerCx,
}

fn Index(cx: Scope<IndexProps>) -> Element {
    let IndexProps { sx, .. } = cx.props;
    let ServerCx { liveview_js, .. } = sx;
    cx.render(rsx! {
        "<!DOCTYPE html>"
        "<html lang=en>"
            head {
                title { "updown" }
                meta { charset: "utf-8" }
                meta { name: "viewport", content:"width=device-width" }
                link { rel: "stylesheet", href: "/tw.css" }
            }
            body {
                div { id: "main" }
                "{liveview_js}"
            }
        "</html>"
    })
}

#[handler]
fn index(req: &mut Request, res: &mut Response, depot: &mut Depot) -> Result<()> {
    let ws_addr = "ws://localhost:9001/ws".to_string();
    let liveview_js = dioxus_liveview::interpreter_glue(&ws_addr);
    let sx = ServerCx {
        ws_addr,
        liveview_js,
    };
    res.render(Text::Html(render_lazy(rsx! {
        Index {
            sx: sx
        }
    })));
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
        h1 { "count: {count}" }
        h2 { "this was added with hot reloading" }
        button { onclick: inc, "inc"  }
        button { onclick: dec, "dec"  }
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
