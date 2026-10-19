-- SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
-- SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
-- SPDX-FileCopyrightText: Copyright Contributors to the Exchange8:Margin
-- SPDX-License-Identifier: Apache-2.0

-- Ex8:Margin - derivatives: contracts, positions, margin accounts, funding.
--
-- Design: design/sentin8-exchange8-data-model-technical-design.md §3.6 (MARGIN).
-- No migration code (new platform): `sentin8-cli vertical init` applies this.

CREATE SCHEMA IF NOT EXISTS margin;

-- Derivative contract master (futures/perpetuals/options).
CREATE TABLE IF NOT EXISTS margin.contract (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id          UUID NOT NULL,
    symbol          TEXT NOT NULL,
    underlying_security UUID,
    contract_type   TEXT NOT NULL CHECK (contract_type IN
                    ('future','perpetual','option')),
    option_type     TEXT CHECK (option_type IN ('call','put')),
    exercise_style  TEXT CHECK (exercise_style IN ('european','american')),
    strike          NUMERIC(18,8),
    expiry_date     DATE,
    multiplier      NUMERIC(18,8) NOT NULL DEFAULT 1,
    settlement_type  TEXT NOT NULL DEFAULT 'cash'
                    CHECK (settlement_type IN ('cash','physical')),
    margin_model    TEXT NOT NULL DEFAULT 'isolated'
                    CHECK (margin_model IN ('isolated','cross','span','portfolio')),
    allocation_method TEXT NOT NULL DEFAULT 'fifo'
                    CHECK (allocation_method IN
                    ('fifo','pro_rata_round_lot','pro_rata_fifo_remainder')),
    UNIQUE (env_id, symbol)
);
CREATE INDEX IF NOT EXISTS idx_exchange8_contract_env ON margin.contract (env_id);

-- Per-account net position per contract (event-sourced from the trade stream).
-- Named contract_position: `position` is a reserved SQL keyword and breaks
-- unqualified table references in RLS/trigger bodies.
CREATE TABLE IF NOT EXISTS margin.contract_position (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    account_id  TEXT NOT NULL,
    contract_id UUID NOT NULL REFERENCES margin.contract(id),
    qty         NUMERIC(18,8) NOT NULL DEFAULT 0,   -- signed: +long / −short
    avg_entry   NUMERIC(18,8) NOT NULL DEFAULT 0,
    realized_pnl NUMERIC(20,8) NOT NULL DEFAULT 0,
    UNIQUE (env_id, account_id, contract_id)
);
CREATE INDEX IF NOT EXISTS idx_exchange8_position_env ON margin.contract_position (env_id);

-- Margin balance + equity + maintenance level per account.
CREATE TABLE IF NOT EXISTS margin.margin_account (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id          UUID NOT NULL,
    account_id      TEXT NOT NULL,
    equity          NUMERIC(20,8) NOT NULL DEFAULT 0,
    maint_bps       INTEGER NOT NULL DEFAULT 5000,   -- 100% = 10000 bps
    init_bps        INTEGER NOT NULL DEFAULT 10000,
    last_mark       NUMERIC(18,8),
    last_mark_ts    TIMESTAMPTZ,
    UNIQUE (env_id, account_id)
);

-- Margin call / liquidation events.
CREATE TABLE IF NOT EXISTS margin.margin_call (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    account_id  TEXT NOT NULL,
    contract_id UUID,
    requirement NUMERIC(20,8) NOT NULL,
    shortfall   NUMERIC(20,8) NOT NULL,
    mark        NUMERIC(18,8) NOT NULL,
    call_ts     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS margin.liquidation (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    account_id  TEXT NOT NULL,
    contract_id UUID,
    close_qty   NUMERIC(18,8) NOT NULL,
    close_is_buy BOOLEAN NOT NULL,
    shortfall   NUMERIC(20,8) NOT NULL,
    mark        NUMERIC(18,8) NOT NULL,
    state       TEXT NOT NULL DEFAULT 'pending'
                CHECK (state IN ('pending','submitted','filled','failed')),
    ts          TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Perpetual funding history.
CREATE TABLE IF NOT EXISTS margin.funding_rate (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    contract_id UUID NOT NULL REFERENCES margin.contract(id),
    premium_index_bps BIGINT NOT NULL,
    cap_bps     BIGINT NOT NULL,
    rate_bps    BIGINT NOT NULL,          -- clamped applied rate
    applied_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Per-account option positions.
CREATE TABLE IF NOT EXISTS margin.option_position (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    account_id  TEXT NOT NULL,
    contract_id UUID NOT NULL REFERENCES margin.contract(id),
    long_qty    NUMERIC(18,8) NOT NULL DEFAULT 0,
    short_qty   NUMERIC(18,8) NOT NULL DEFAULT 0,
    avg_premium NUMERIC(18,8) NOT NULL DEFAULT 0
);

-- Cached Greeks per option.
CREATE TABLE IF NOT EXISTS margin.greeks_snapshot (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    contract_id UUID NOT NULL REFERENCES margin.contract(id),
    delta       NUMERIC(18,8),
    gamma       NUMERIC(18,8),
    theta       NUMERIC(18,8),
    vega        NUMERIC(18,8),
    rho         NUMERIC(18,8),
    mark        NUMERIC(18,8),
    ts          TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- RLS (architecture-principles.md §4.3): enable + force on all tenant tables.
-- Policies use the platform helpers sentin8_context_initialized() and
-- sentin8_current_env_id(); services call set_tenant_context on the same
-- connection as the data query.
-- ---------------------------------------------------------------------------
DO $$
DECLARE tbl TEXT;
BEGIN
    FOREACH tbl IN ARRAY ARRAY['margin.contract', 'margin.contract_position', 'margin.margin_account', 'margin.margin_call', 'margin.liquidation', 'margin.funding_rate', 'margin.option_position', 'margin.greeks_snapshot'] LOOP
        BEGIN
            EXECUTE format('ALTER TABLE %s ENABLE ROW LEVEL SECURITY', tbl);
            EXECUTE format('ALTER TABLE %s FORCE ROW LEVEL SECURITY', tbl);
            EXECUTE format('DROP POLICY IF EXISTS %1$I_select ON %2$s;
CREATE POLICY %1$I_select ON %2$s FOR SELECT USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())', split_part(tbl, '.', 2), tbl);
            EXECUTE format('DROP POLICY IF EXISTS %1$I_insert ON %2$s;
CREATE POLICY %1$I_insert ON %2$s FOR INSERT WITH CHECK (sentin8_context_initialized() AND env_id = sentin8_current_env_id())', split_part(tbl, '.', 2), tbl);
            EXECUTE format('DROP POLICY IF EXISTS %1$I_update ON %2$s;
CREATE POLICY %1$I_update ON %2$s FOR UPDATE USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())', split_part(tbl, '.', 2), tbl);
            EXECUTE format('DROP POLICY IF EXISTS %1$I_delete ON %2$s;
CREATE POLICY %1$I_delete ON %2$s FOR DELETE USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())', split_part(tbl, '.', 2), tbl);
        EXCEPTION WHEN OTHERS THEN
            RAISE NOTICE 'RLS setup failed for %: %', tbl, SQLERRM;
        END;
    END LOOP;
END $$;

-- ---------------------------------------------------------------------------
-- Per-service role grants (schema-per-service: this service owns schema
-- `margin`; grants live in db `exchange8` only - design doc §1).
-- ---------------------------------------------------------------------------
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'exchange8_margin_service') THEN
        ALTER ROLE exchange8_margin_service SET search_path = margin, public;
        GRANT USAGE ON SCHEMA margin TO exchange8_margin_service;
        GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA margin TO exchange8_margin_service;
        GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA margin TO exchange8_margin_service;
    END IF;
END $$;

-- ---------------------------------------------------------------------------
-- Local outbox (DBX §6.7 - generated from `sentin8_outbox::outbox_table_sql(
-- "margin")`): rows written in the same DB transaction as the domain change;
-- the shell's relay (boot step §8b) pushes them to Bus `POST /v1/produce`.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS margin.outbox (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id          UUID NOT NULL,
    event_type      TEXT NOT NULL,
    stream_family   TEXT NOT NULL,
    payload         JSONB NOT NULL,
    publisher_id_token  TEXT,
    publisher_subject   TEXT,
    relayed         BOOLEAN NOT NULL DEFAULT false,
    relayed_at      TIMESTAMPTZ,
    relay_attempts  INT NOT NULL DEFAULT 0,
    next_retry_at   TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_margin_outbox_unrelayed
    ON margin.outbox (created_at) WHERE relayed = false;
DO $$ BEGIN
    EXECUTE 'ALTER TABLE margin.outbox ENABLE ROW LEVEL SECURITY';
    EXECUTE 'ALTER TABLE margin.outbox FORCE ROW LEVEL SECURITY';
    EXECUTE 'DROP POLICY IF EXISTS outbox_select ON margin.outbox';
    EXECUTE 'CREATE POLICY outbox_select ON margin.outbox FOR SELECT
        USING (sentin8_context_initialized()
               AND env_id = sentin8_current_env_id())';
    EXECUTE 'DROP POLICY IF EXISTS outbox_insert ON margin.outbox';
    EXECUTE 'CREATE POLICY outbox_insert ON margin.outbox FOR INSERT
        WITH CHECK (sentin8_context_initialized()
               AND env_id = sentin8_current_env_id())';
    EXECUTE 'DROP POLICY IF EXISTS outbox_update ON margin.outbox';
    EXECUTE 'CREATE POLICY outbox_update ON margin.outbox FOR UPDATE
        USING (sentin8_context_initialized()
               AND env_id = sentin8_current_env_id())';
EXCEPTION WHEN OTHERS THEN
    RAISE NOTICE 'RLS setup failed for margin.outbox: %', SQLERRM;
END $$;

CREATE OR REPLACE FUNCTION margin.margin_notify_outbox() RETURNS TRIGGER
LANGUAGE plpgsql AS $fn$
BEGIN
    PERFORM pg_notify('margin_outbox_changed', '');
    RETURN NEW;
END;
$fn$;
DROP TRIGGER IF EXISTS outbox_notify_trigger ON margin.outbox;
CREATE TRIGGER outbox_notify_trigger
    AFTER INSERT ON margin.outbox
    FOR EACH ROW
    EXECUTE FUNCTION margin.margin_notify_outbox();

-- ---------------------------------------------------------------------------
-- Local audit journal (DBX §6.8 - generated from
-- `sentin8_audit::audit_journal_sql("margin")`): rows written in the same
-- transaction as the domain change; the shell's pusher (boot step §8c) pushes
-- them to Sys `POST /v1/audit` (`sentin8:sys:audit:write`). A Sys outage
-- never drops audit.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS margin.audit_journal (
    id              UUID PRIMARY KEY,
    env_id          UUID NOT NULL,
    action          TEXT NOT NULL,
    target_resource_type TEXT,
    target_resource_id    TEXT,
    actor_user_id   TEXT,
    actor_subject_type TEXT,
    severity        TEXT NOT NULL DEFAULT 'info',
    correlation_id  TEXT,
    before_state    JSONB,
    after_state     JSONB,
    details         JSONB,
    pushed          BOOLEAN NOT NULL DEFAULT false,
    pushed_at       TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_margin_audit_journal_unpushed
    ON margin.audit_journal (created_at) WHERE pushed = false;
DO $$ BEGIN
    EXECUTE 'ALTER TABLE margin.audit_journal ENABLE ROW LEVEL SECURITY';
    EXECUTE 'ALTER TABLE margin.audit_journal FORCE ROW LEVEL SECURITY';
    EXECUTE 'DROP POLICY IF EXISTS audit_journal_dml ON margin.audit_journal';
    EXECUTE 'CREATE POLICY audit_journal_dml ON margin.audit_journal
        USING (sentin8_context_initialized()
               AND env_id = sentin8_current_env_id())
        WITH CHECK (sentin8_context_initialized()
               AND env_id = sentin8_current_env_id())';
EXCEPTION WHEN OTHERS THEN
    RAISE NOTICE 'RLS setup failed for margin.audit_journal: %', SQLERRM;
END $$;

-- Outbox + journal role grants (same pattern as the table grants above).
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'exchange8_margin_service') THEN
        GRANT SELECT, INSERT, UPDATE ON ALL TABLES IN SCHEMA margin TO exchange8_margin_service;
    END IF;
END $$;
