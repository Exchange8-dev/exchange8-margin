// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Exchange8:Margin
// SPDX-License-Identifier: Apache-2.0

//! OpenAPI spec for Exchange8:Margin.
//!
//! utoipa annotations on the service's handlers + the shared Shield
//! policy-admin surface (`sentin8::shell::policy_*` wrappers); the
//! shell-owned `/v1/health` mount is described through a spec wrapper
//! (`sentin8/design/sentin8-openapi-coverage-design.md` §2).

use utoipa::OpenApi;

pub mod w {

    /// Spec wrapper for the shell-owned `/v1/health` mount.
    #[utoipa::path(
        get,
        path = "/v1/health",
        responses(
            (status = 200, description = "Service is healthy", body = sentin8::middleware::health::LiveHealth),
        ),
        security(),
        tag = "margin",
    )]
    pub async fn margin_health() {}
    /// Spec wrapper for the alias route (same handler as the public path).
    #[utoipa::path(
        get,
        path = "/v1/admin/positions",
        responses(
            (status = 200, description = "OK", body = Vec<crate::handlers::PositionResponse>),
            (status = 403, description = "Forbidden"),
        ),
        security(),
        tag = "exchange8-admin",
    )]
    pub async fn admin_list_positions() {}

    /// Spec wrapper for the alias route (same handler as the public path).
    #[utoipa::path(
        get,
        path = "/v1/admin/margin-accounts",
        responses(
            (status = 200, description = "OK", body = Vec<crate::handlers::MarginAccountResponse>),
            (status = 403, description = "Forbidden"),
        ),
        security(),
        tag = "exchange8-admin",
    )]
    pub async fn admin_list_margin_accounts() {}

    /// Spec wrapper for the alias route (same handler as the public path).
    #[utoipa::path(
        get,
        path = "/v1/admin/liquidations",
        responses(
            (status = 200, description = "OK", body = Vec<crate::handlers::LiquidationResponse>),
            (status = 403, description = "Forbidden"),
        ),
        security(),
        tag = "exchange8-admin",
    )]
    pub async fn admin_list_liquidations() {}
}

/// OpenAPI spec for the margin API.
#[derive(OpenApi)]
#[openapi(
    paths(
        crate::openapi::w::admin_list_positions,
        crate::openapi::w::admin_list_margin_accounts,
        crate::openapi::w::admin_list_liquidations,
crate::handlers::list_positions,
crate::handlers::list_margin_accounts,
crate::handlers::list_liquidations,
crate::openapi::w::margin_health,
        sentin8::shell::policy_list_sets,
        sentin8::shell::policy_get_set,
        sentin8::shell::policy_put_set,
        sentin8::shell::policy_delete_set,
        sentin8::shell::policy_test_set,
        sentin8::shell::policy_mode_enforce,
        sentin8::shell::policy_mode_shadow,
        sentin8::shell::policy_mode_disable,
        sentin8::shell::policy_create_set,
        sentin8::shell::policy_save_set,
        sentin8::shell::policy_flip_set,
        sentin8::shell::policy_list_decisions,
        sentin8::shell::policy_delete_relation_path,
        sentin8::shell::policy_delete_attribute_path,
        sentin8::shell::policy_list_relations,
        sentin8::shell::policy_put_relation,
        sentin8::shell::policy_delete_relation,
        sentin8::shell::policy_check_relation,
        sentin8::shell::policy_list_attributes,
        sentin8::shell::policy_put_attribute,
        sentin8::shell::policy_delete_attribute,
    ),
    components(schemas(
        crate::handlers::PositionResponse,
        crate::handlers::MarginAccountResponse,
        crate::handlers::LiquidationResponse,
        sentin8::middleware::health::LiveHealth,
    )),
    security(),
    tags(
        (name = "margin", description = "Exchange8:Margin"),
        (name = "exchange8-admin", description = "Exchange8 admin surface (RFW)"),
        (name = "policy", description = "Shield policy administration (shared surface)"),
    ),
)]
pub struct MarginOpenApi;
