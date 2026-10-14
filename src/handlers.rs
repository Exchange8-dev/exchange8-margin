// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Exchange8:Margin
// SPDX-License-Identifier: Apache-2.0

//! Ex8:Margin data handlers — positions + margin accounts query API.
//!
//! Every handler follows the platform authorization pipeline (§5): verify
//! bearer → scope check → RLS context (same connection) → explicit
//! `WHERE env_id` (defense-in-depth) → query.
//!
//! Design: `design/sentin8-exchange8-margin-technical-design.md` §4
//! + data-model doc §3.6 (MARGIN-owned `margin.contract_position` /
//! `margin.margin_account`); detailed design: `services/margin/DESIGN.md`
//! §4 (API surface) and §5 (cross-cutting concerns).

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::Acquire;
use uuid::Uuid;

use sentin8::shell::{err_db, AppState};

/// Scope constants for every handler in this module — single source of truth
/// for both the call sites and the authorization matrix tests (see
/// `services/margin/DESIGN.md` §4, §9).
pub mod scopes {
    /// `GET /v1/positions`.
    pub const POSITIONS_READ: &str = "exchange8:position:read";
    /// `GET /v1/margin-accounts` + `GET /v1/liquidations`.
    pub const MARGIN_READ: &str = "exchange8:margin:read";
}

#[derive(Debug, Deserialize)]
pub struct ListPositionsQuery {
    /// Filter by trading account.
    pub account_id: Option<String>,
    /// Filter by contract UUID.
    pub contract_id: Option<Uuid>,
    /// Only non-flat positions.
    pub open_only: Option<bool>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PositionResponse {
    pub id: Uuid,
    pub env_id: Uuid,
    pub account_id: String,
    pub contract_id: Uuid,
    /// Signed: +long / −short.
    pub qty: rust_decimal::Decimal,
    pub avg_entry: rust_decimal::Decimal,
    pub realized_pnl: rust_decimal::Decimal,
}

/// `GET /v1/positions` — net positions per (account, contract).
pub async fn list_positions(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<ListPositionsQuery>,
) -> Result<Json<Vec<PositionResponse>>, (StatusCode, String)> {
    let a = sentin8::shell::verify_token(&state.config, &headers).await?;
    sentin8::shell::policy::require(
        &state.admin.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("exchange8:position:read"),
    )
    .await?;
    let mut conn = state.pool.acquire().await.map_err(err_db)?;
    let mut ctx = conn.begin().await.map_err(err_db)?;
    sentin8::shell::set_tenant_context(&mut ctx, a.env_id()).await;
    let limit = q.limit.unwrap_or(100).min(500);
    let rows = sqlx::query_as::<_, PositionResponse>(
        r#"SELECT id, env_id, account_id, contract_id, qty, avg_entry, realized_pnl
             FROM margin.contract_position
            WHERE env_id = $1
              AND ($2::text IS NULL OR account_id = $2)
              AND ($3::uuid IS NULL OR contract_id = $3)
              AND ($4::bool = false OR qty <> 0)
            ORDER BY account_id, contract_id LIMIT $5"#,
    )
    .bind(a.env_id())
    .bind(q.account_id.as_deref())
    .bind(q.contract_id)
    .bind(q.open_only.unwrap_or(false))
    .bind(limit)
    .fetch_all(&mut *ctx)
    .await
    .map_err(err_db)?;
    ctx.commit().await.map_err(err_db)?;
    Ok(Json(rows))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MarginAccountResponse {
    pub id: Uuid,
    pub env_id: Uuid,
    pub account_id: String,
    pub equity: rust_decimal::Decimal,
    pub maint_bps: i32,
    pub init_bps: i32,
    pub last_mark: Option<rust_decimal::Decimal>,
    pub last_mark_ts: Option<chrono::DateTime<chrono::Utc>>,
}

/// `GET /v1/margin-accounts` — margin balances + maintenance levels.
pub async fn list_margin_accounts(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Vec<MarginAccountResponse>>, (StatusCode, String)> {
    let a = sentin8::shell::verify_token(&state.config, &headers).await?;
    sentin8::shell::policy::require(
        &state.admin.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("exchange8:margin:read"),
    )
    .await?;
    let mut conn = state.pool.acquire().await.map_err(err_db)?;
    let mut ctx = conn.begin().await.map_err(err_db)?;
    sentin8::shell::set_tenant_context(&mut ctx, a.env_id()).await;
    let rows = sqlx::query_as::<_, MarginAccountResponse>(
        r#"SELECT id, env_id, account_id, equity, maint_bps, init_bps,
                  last_mark, last_mark_ts
             FROM margin.margin_account
            WHERE env_id = $1
            ORDER BY account_id LIMIT 500"#,
    )
    .bind(a.env_id())
    .fetch_all(&mut *ctx)
    .await
    .map_err(err_db)?;
    ctx.commit().await.map_err(err_db)?;
    Ok(Json(rows))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct LiquidationResponse {
    pub id: Uuid,
    pub env_id: Uuid,
    pub account_id: String,
    pub contract_id: Option<Uuid>,
    pub close_qty: rust_decimal::Decimal,
    pub close_is_buy: bool,
    pub shortfall: rust_decimal::Decimal,
    pub state: String,
    pub ts: chrono::DateTime<chrono::Utc>,
}

/// `GET /v1/liquidations` — liquidation events.
pub async fn list_liquidations(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Vec<LiquidationResponse>>, (StatusCode, String)> {
    let a = sentin8::shell::verify_token(&state.config, &headers).await?;
    sentin8::shell::policy::require(
        &state.admin.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("exchange8:margin:read"),
    )
    .await?;
    let mut conn = state.pool.acquire().await.map_err(err_db)?;
    let mut ctx = conn.begin().await.map_err(err_db)?;
    sentin8::shell::set_tenant_context(&mut ctx, a.env_id()).await;
    let rows = sqlx::query_as::<_, LiquidationResponse>(
        r#"SELECT id, env_id, account_id, contract_id, close_qty, close_is_buy,
                  shortfall, state, ts
             FROM margin.liquidation
            WHERE env_id = $1
            ORDER BY ts DESC LIMIT 200"#,
    )
    .bind(a.env_id())
    .fetch_all(&mut *ctx)
    .await
    .map_err(err_db)?;
    ctx.commit().await.map_err(err_db)?;
    Ok(Json(rows))
}

// ---------------------------------------------------------------------------
// Authorization matrix tests (see services/margin/DESIGN.md §9). DB-free: the
// decision the handlers delegate to (`AuthContext::require_any_scope`) is
// exercised against the SAME constants the handlers require.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod scope_matrix {
    use super::scopes;
    use sentin8::shell::{AuthContext, VerifiedClaims};

    const HANDLER_SCOPES: &[(&str, &[&str])] = &[
        ("list_positions", &[scopes::POSITIONS_READ]),
        ("list_margin_accounts", &[scopes::MARGIN_READ]),
        ("list_liquidations", &[scopes::MARGIN_READ]),
    ];

    const SIBLING_SCOPES: &[&str] = &[scopes::POSITIONS_READ, scopes::MARGIN_READ];

    fn ctx(scope: &str) -> AuthContext {
        AuthContext {
            claims: VerifiedClaims {
                sub: "matrix-test".into(),
                iss: "https://auth.test".into(),
                aud: "exchange8".into(),
                jti: "matrix-jti".into(),
                iat: 0,
                exp: i64::MAX,
                token_tp: "access".into(),
                scope: scope.into(),
                env_id: Some("00000000-0000-0000-0000-000000000002".into()),
                email: None,
                act: None,
                vertical: None,
                service_type: None,
            },
            // Bearer-path context: no certificate workload on the test
            // identity (CERTFREE-4V — the field exists for the cert path).
            workload: None,
        }
    }

    #[test]
    fn every_handler_denies_without_and_with_wrong_scope() {
        for (handler, required) in HANDLER_SCOPES {
            let err = ctx("")
                .require_any_scope(required)
                .expect_err("no scope must deny");
            assert_eq!(err.0.as_u16(), 403, "{handler}: expected FORBIDDEN");

            let err = ctx("bank8:ledger:read")
                .require_any_scope(required)
                .expect_err("foreign scope must deny");
            assert_eq!(err.0.as_u16(), 403, "{handler}: foreign must be FORBIDDEN");

            for sibling in SIBLING_SCOPES {
                if required.contains(sibling) {
                    continue;
                }
                let err = ctx(sibling)
                    .require_any_scope(required)
                    .expect_err("sibling scope must deny");
                assert_eq!(
                    err.0.as_u16(),
                    403,
                    "{handler}: sibling {sibling} must be FORBIDDEN"
                );
            }
        }
    }

    #[test]
    fn every_handler_allows_each_required_scope_and_platform_admin() {
        for (handler, required) in HANDLER_SCOPES {
            for scope in *required {
                assert!(
                    ctx(scope).require_any_scope(required).is_ok(),
                    "{handler}: exact scope {scope} must allow"
                );
            }
            let all = required.join(" ");
            assert!(ctx(&all).require_any_scope(required).is_ok(), "{handler}");
            assert!(
                ctx("sentin8:sys:admin").require_any_scope(required).is_ok(),
                "{handler}"
            );
        }
    }

    #[test]
    fn grants_are_prefix_permissive_and_denials_are_exact() {
        assert!(ctx("exchange8:position:read:all")
            .require_any_scope(&[scopes::POSITIONS_READ])
            .is_ok());
        assert!(ctx("exchange8:position:readx")
            .require_any_scope(&[scopes::POSITIONS_READ])
            .is_err());
    }

    #[test]
    fn scope_constants_follow_the_naming_convention() {
        // Registered contract: 3-4 segments (see services/margin/DESIGN.md §4).
        let mut all: Vec<&str> = Vec::new();
        for (handler, required) in HANDLER_SCOPES {
            for scope in *required {
                all.push(scope);
                let segs: Vec<_> = scope.split(':').collect();
                assert!(
                    (3..=4).contains(&segs.len()),
                    "{handler}: {scope} must have 3-4 segments"
                );
                assert_eq!(segs[0], "exchange8", "{handler}: {scope} wrong vertical");
                assert!(
                    segs.iter().all(|s| !s.is_empty()
                        && s.chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())),
                    "{handler}: {scope} segments must be lowercase alnum"
                );
            }
        }
        // Every declared constant must be wired into HANDLER_SCOPES. Shared
        // constants across handlers are legitimate — dedup first.
        all.sort_unstable();
        all.dedup();
        let mut declared = vec![scopes::POSITIONS_READ, scopes::MARGIN_READ];
        declared.sort_unstable();
        assert_eq!(all, declared, "scopes module and HANDLER_SCOPES diverged");
    }
}
