// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Exchange8:Margin
// SPDX-License-Identifier: Apache-2.0

//! Exchange8:Margin HTTP handler tests (design §13b.1; see DESIGN.md §9).
//!
//! Drives the REAL router through `tower::ServiceExt::oneshot`: route
//! mounting, the authorization pipeline (bearer → scope), status codes,
//! and the JSON error envelope - without a database. DB-free guarantee:
//! every data-path handler calls `require_any_scope` BEFORE the first
//! pool access, so a bearer-less request never reaches `state.pool`.
//!
//! The authorized data paths are covered by `tests/sql_matrix.rs`
//! + the scope matrices inside `handlers.rs` (see DESIGN.md §9).
//!
//! The margin crate is bin-only, so the handlers module is pulled in
//! via `#[path]` - the same pattern as trade8's desk tests.

#[path = "../src/handlers.rs"]
mod handlers;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

/// Minimal router mirroring `main.rs::extend_router` (same paths/methods).
fn router() -> axum::Router {
    axum::Router::new()
        .route(
            "/v1/positions",
            axum::routing::get(handlers::list_positions),
        )
        .route(
            "/v1/margin-accounts",
            axum::routing::get(handlers::list_margin_accounts),
        )
        .route(
            "/v1/liquidations",
            axum::routing::get(handlers::list_liquidations),
        )
        .with_state(test_state())
}

/// Install the platform's PQ TLS provider (services do this at boot;
/// reqwest inside the auth client requires a configured provider).
fn install_crypto() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = sentin8::tls::crypto::sentin8_pq_provider().install_default();
    });
}

/// `ServiceCore` with a never-connected pool. Safe: the bearer-less
/// requests these tests send are rejected by `verify_bearer_token` before
/// any pool access.
fn test_state() -> Arc<sentin8::shell::AppState> {
    install_crypto();
    let config = Arc::new(sentin8::shell::ShellConfig {
        service_type: "exchange8:margin",
        port: 0,
        database_url: "postgresql://invalid:invalid@127.0.0.1:5/none".into(),
        sentin8_auth_issuer: "https://auth.invalid".into(),
        sentin8_bus_url: String::new(),
        sentin8_auth_client: sentin8::auth::client::Client::new(
            sentin8::auth::client::Client::builder()
                .base_url("https://auth.invalid")
                .build()
                .expect("client options are valid"),
        ),
        valkey_url: String::new(),
    });
    let pool = sqlx::PgPool::connect_lazy("postgresql://invalid:invalid@127.0.0.1:5/none")
        .expect("lazy pool url is parseable");
    Arc::new(sentin8::shell::AppState {
        config,
        pool: pool.clone(),
        admin: sentin8::shield::admin::SharedAdminState::test("exchange8:margin", pool),
    })
}

fn req(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "exchange8-test")
        .body(Body::empty())
        .unwrap()
}

/// 1 - every route rejects a bearer-less request with 401 (auth pipeline
/// before any DB work).
#[tokio::test]
async fn every_route_rejects_unauthenticated_with_401() {
    let app = router();
    for (method, uri) in [
        ("GET", "/v1/positions"),
        ("GET", "/v1/margin-accounts"),
        ("GET", "/v1/liquidations"),
    ] {
        let res = app.clone().oneshot(req(method, uri)).await.unwrap();
        let st = res.status();
        // JSON-extractor routes (e.g. POST with axum::Json) reject the empty
        // body with 415 BEFORE the handler body (and its auth gate) runs.
        // Both 401 (auth gate first) and 415 (body extractor first) prove the
        // request never reached the DB layer.
        assert!(
            st == StatusCode::UNAUTHORIZED || st == StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "{method} {uri}: expected 401 or 415, got {st}"
        );
    }
}

/// 2 - wrong-method on an existing path → 405 (route table shape check).
#[tokio::test]
async fn wrong_method_gets_405_not_404() {
    let app = router();
    let res = app.oneshot(req("DELETE", "/v1/positions")).await.unwrap();
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
}

/// 3 - unmatched path → 404.
#[tokio::test]
async fn unknown_path_gets_404() {
    let app = router();
    let res = app.oneshot(req("GET", "/v1/nope")).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// 4 - error envelope: plain-text auth message (never SQL internals).
#[tokio::test]
async fn unauthenticated_error_envelope_is_plain_text_401() {
    let app = router();
    let res = app.oneshot(req("GET", "/v1/positions")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("missing Authorization header"),
        "error body must carry the auth message, got: {text}"
    );
}
