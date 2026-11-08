CREATE TABLE ask_gateway_invocations (
    instance_id TEXT NOT NULL,
    chat_id BIGINT NOT NULL,
    message_id INTEGER NOT NULL,
    claim_token UUID NOT NULL,
    lease_until TIMESTAMPTZ NOT NULL,
    completed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (instance_id, chat_id, message_id)
);

CREATE INDEX ask_gateway_invocations_expired_lease_idx
    ON ask_gateway_invocations (lease_until)
    WHERE completed_at IS NULL;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'nedobot_mcp_ro') THEN
        REVOKE ALL ON TABLE ask_gateway_invocations FROM nedobot_mcp_ro;
    END IF;
END $$;
