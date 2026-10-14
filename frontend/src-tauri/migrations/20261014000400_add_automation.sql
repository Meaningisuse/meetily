CREATE TABLE automation_clients (
 id TEXT PRIMARY KEY, name TEXT NOT NULL, token_hash BLOB NOT NULL UNIQUE CHECK(length(token_hash)=32),
 scopes_json TEXT NOT NULL, secret_ref TEXT NOT NULL UNIQUE,
 expires_at INTEGER NOT NULL, revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN(0,1)), created_at INTEGER NOT NULL
);
CREATE TABLE automation_webhooks (
 id TEXT PRIMARY KEY, url TEXT NOT NULL, secret_ref TEXT NOT NULL UNIQUE,
 allowed_hosts_json TEXT NOT NULL, kinds_json TEXT NOT NULL,
 enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN(0,1)), created_at INTEGER NOT NULL
);
CREATE TABLE automation_events (
 id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, kind TEXT NOT NULL,
 revision INTEGER NOT NULL, job_id TEXT, occurred_at TEXT NOT NULL, payload_json TEXT NOT NULL
);
CREATE TABLE automation_deliveries (
 id TEXT PRIMARY KEY, event_id TEXT NOT NULL REFERENCES automation_events(id) ON DELETE CASCADE,
 webhook_id TEXT NOT NULL REFERENCES automation_webhooks(id) ON DELETE CASCADE,
 attempts INTEGER NOT NULL DEFAULT 0, next_attempt_at INTEGER NOT NULL,
 status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN('pending','delivering','succeeded','failed','cancelled')),
 last_status INTEGER, UNIQUE(event_id,webhook_id)
);
CREATE INDEX automation_delivery_due ON automation_deliveries(status,next_attempt_at);
CREATE TABLE automation_requests (
 client_id TEXT NOT NULL REFERENCES automation_clients(id) ON DELETE CASCADE, key TEXT NOT NULL,
 request_hash TEXT NOT NULL, response_json TEXT, expires_at INTEGER NOT NULL,
 created_at INTEGER NOT NULL, PRIMARY KEY(client_id,key)
);

CREATE TABLE automation_config (id INTEGER PRIMARY KEY CHECK(id=1),enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN(0,1)));
INSERT INTO automation_config(id,enabled) VALUES(1,0);
