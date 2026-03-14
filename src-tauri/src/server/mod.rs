pub mod routes;
pub mod ws;

use axum::{routing::get, routing::post, Router};
use tower_http::cors::CorsLayer;

use crate::state::AppState;

pub async fn start_server(state: AppState) -> anyhow::Result<u16> {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    *state.server_port.write().await = Some(port);

    let app = build_router(state);

    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .ok();
    });

    Ok(port)
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(serve_guest_ui))
        .route("/api/join", post(routes::join))
        .route("/api/queue", get(routes::get_queue))
        .route("/api/search", get(routes::search))
        .route("/api/request", post(routes::request_track))
        .route("/api/vote", post(routes::vote))
        .route("/ws", get(ws::ws_handler))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn serve_guest_ui() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../../assets/guest.html"))
}
