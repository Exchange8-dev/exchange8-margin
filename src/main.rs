// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Exchange8:Margin
// SPDX-License-Identifier: Apache-2.0

//! Exchange8:MARGIN service binary — derivatives positions, margin, liquidation, funding, Greeks.
//!
//! Design: `design/sentin8-exchange8-margin-technical-design.md`;
//! detailed design: `services/margin/DESIGN.md` §2. Core logic in the
//! sibling modules; this shell wires the tokio HTTP/TLS surface via the
//! platform `sentin8-shell` crate (registration, DB pool, health).

#![allow(dead_code)]

// (master design §11.1). Workers (bus consumers, session
// supervisors, purge jobs) are in-process tokio tasks, so the
mod expiry;
mod greeks;
mod handlers;
mod margin;
mod registration;

fn extend_router(
    app: axum::Router<std::sync::Arc<sentin8::shell::AppState>>,
    _state: std::sync::Arc<sentin8::shell::AppState>,
) -> axum::Router<std::sync::Arc<sentin8::shell::AppState>> {
    // ── Sentin8:Policy admin API (shared implementation;
    // the shell AppState already implements HasPolicyAdmin).
    sentin8::shell::mount_policy_admin(
        // MTM/funding run on Sentin8:Cron + index events (design §4).
        // The HTTP router exposes the positions/margin query API (RLS-scoped)
        // + health; admin/RFW data endpoints alias the same handlers.
        app.route(
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
        .route(
            "/v1/admin/positions",
            axum::routing::get(handlers::list_positions),
        )
        .route(
            "/v1/admin/margin-accounts",
            axum::routing::get(handlers::list_margin_accounts),
        )
        .route(
            "/v1/admin/liquidations",
            axum::routing::get(handlers::list_liquidations),
        ),
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    sentin8::shell::serve(
        sentin8::shell::Sentin8Shell::new("exchange8:margin", "EXCHANGE8_MARGIN")
            .with_registration(registration::print)
            .with_simple_state()
            .with_router(extend_router),
    )
    .await
}
