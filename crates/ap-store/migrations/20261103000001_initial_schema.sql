-- AccessPilot Initial Schema

CREATE TYPE purchase_status AS ENUM (
    'AWAITING_APPROVAL',
    'RESERVED',
    'PENDING_UNKNOWN',
    'CAPTURED',
    'RELEASED',
    'DENIED'
);

CREATE TABLE IF NOT EXISTS tenants (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS api_keys (
    hashed_key BYTEA PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants(id),
    scope TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS policies (
    version INT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants(id),
    policy_json JSONB NOT NULL,
    state TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS tasks (
    id UUID PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES tenants(id),
    goal TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'running',
    policy_version INT NOT NULL,
    autonomy TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS budgets (
    task_id UUID PRIMARY KEY REFERENCES tasks(id),
    currency TEXT NOT NULL,
    cap_minor BIGINT NOT NULL CHECK (cap_minor >= 0),
    reserved_minor BIGINT NOT NULL DEFAULT 0 CHECK (reserved_minor >= 0),
    captured_minor BIGINT NOT NULL DEFAULT 0 CHECK (captured_minor >= 0),
    version BIGINT NOT NULL DEFAULT 0,
    CHECK (reserved_minor + captured_minor <= cap_minor)
);

CREATE TABLE IF NOT EXISTS offers (
    id UUID PRIMARY KEY,
    task_id UUID NOT NULL REFERENCES tasks(id),
    provider_id TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    amount_minor BIGINT NOT NULL CHECK (amount_minor >= 0),
    currency TEXT NOT NULL,
    tax_minor BIGINT NOT NULL DEFAULT 0,
    fee_minor BIGINT NOT NULL DEFAULT 0,
    price_known BOOLEAN NOT NULL DEFAULT true,
    expires_at TIMESTAMPTZ NOT NULL,
    sig TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS purchases (
    id UUID PRIMARY KEY,
    task_id UUID NOT NULL REFERENCES tasks(id),
    offer_id UUID NOT NULL REFERENCES offers(id),
    idempotency_key TEXT NOT NULL,
    amount_minor BIGINT NOT NULL CHECK (amount_minor > 0),
    status purchase_status NOT NULL,
    provider_payment_id TEXT UNIQUE,
    policy_version INT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, status),
    UNIQUE (task_id, idempotency_key)
);

CREATE TABLE IF NOT EXISTS grants (
    id UUID PRIMARY KEY,
    purchase_id UUID NOT NULL UNIQUE,
    purchase_status purchase_status NOT NULL DEFAULT 'CAPTURED'
                    CHECK (purchase_status = 'CAPTURED'),
    token_hash BYTEA NOT NULL,
    scope TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    FOREIGN KEY (purchase_id, purchase_status)
        REFERENCES purchases (id, status) ON UPDATE RESTRICT
);

CREATE TABLE IF NOT EXISTS idempotency_keys (
    tenant_id TEXT NOT NULL REFERENCES tenants(id),
    key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    status TEXT NOT NULL,
    response JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, key)
);

CREATE TABLE IF NOT EXISTS ledger_events (
    task_id UUID NOT NULL REFERENCES tasks(id),
    seq BIGINT NOT NULL,
    type TEXT NOT NULL,
    payload JSONB NOT NULL,
    prev_hash TEXT NOT NULL,
    hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (task_id, seq)
);

CREATE OR REPLACE FUNCTION reject_ledger_mutation()
RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'ledger_events is append-only: UPDATE and DELETE are prohibited';
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_no_ledger_mutation ON ledger_events;
CREATE TRIGGER trg_no_ledger_mutation
BEFORE UPDATE OR DELETE ON ledger_events
FOR EACH ROW EXECUTE FUNCTION reject_ledger_mutation();

CREATE TABLE IF NOT EXISTS webhook_events (
    provider_event_id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    payload JSONB NOT NULL
);

CREATE TABLE IF NOT EXISTS approvals (
    id UUID PRIMARY KEY,
    purchase_id UUID NOT NULL REFERENCES purchases(id),
    requested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    decided_by TEXT,
    decision TEXT,
    decided_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS provider_stats (
    provider_id TEXT PRIMARY KEY,
    success_ewma BIGINT NOT NULL DEFAULT 10000,
    p95_latency_ms BIGINT NOT NULL DEFAULT 100,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS incidents (
    id UUID PRIMARY KEY,
    kind TEXT NOT NULL,
    purchase_id UUID REFERENCES purchases(id),
    opened_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at TIMESTAMPTZ,
    details JSONB NOT NULL
);

