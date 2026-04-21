-- v1 schema — Appendix A of docs/v1-architecture.md.

PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS profiles (
  id                  INTEGER PRIMARY KEY,
  fingerprint         TEXT NOT NULL,
  version             TEXT NOT NULL,
  provenance          TEXT NOT NULL,
  synthesis_source    TEXT,
  status              TEXT NOT NULL,
  dsl_blob_sha256     TEXT NOT NULL,
  rubric_blob_sha256  TEXT,
  parent_fp           TEXT,
  split_on_flag       TEXT,
  created_at          INTEGER NOT NULL,
  last_used_at        INTEGER,
  sample_count        INTEGER NOT NULL DEFAULT 0,
  fidelity_rolling    REAL,
  signature           BLOB,
  signer_key_id       TEXT,
  UNIQUE (fingerprint, version)
);
CREATE INDEX IF NOT EXISTS idx_profiles_fingerprint_active
  ON profiles(fingerprint) WHERE status = 'active';
CREATE INDEX IF NOT EXISTS idx_profiles_status ON profiles(status);

CREATE TABLE IF NOT EXISTS blobs (
  sha256     TEXT PRIMARY KEY,
  kind       TEXT NOT NULL,
  size       INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  ref_count  INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS samples (
  id                  INTEGER PRIMARY KEY,
  fingerprint         TEXT NOT NULL,
  sample_blob_sha256  TEXT NOT NULL,
  exit_code           INTEGER NOT NULL,
  captured_at         INTEGER NOT NULL,
  shape_class         TEXT,
  FOREIGN KEY (sample_blob_sha256) REFERENCES blobs(sha256)
);
CREATE INDEX IF NOT EXISTS idx_samples_fp ON samples(fingerprint);

CREATE TABLE IF NOT EXISTS sessions (
  id                     TEXT PRIMARY KEY,
  fingerprint            TEXT NOT NULL,
  profile_id             INTEGER,
  command                TEXT NOT NULL,
  argv_raw               TEXT NOT NULL,
  cwd                    TEXT,
  parent_agent           TEXT,
  started_at             INTEGER NOT NULL,
  ended_at               INTEGER,
  exit_code              INTEGER,
  raw_blob_sha256        TEXT,
  compressed_blob_sha256 TEXT,
  raw_bytes              INTEGER,
  compressed_bytes       INTEGER,
  tokens_saved_est       INTEGER,
  model_estimator        TEXT,
  mode                   TEXT,
  FOREIGN KEY (profile_id) REFERENCES profiles(id)
);
CREATE INDEX IF NOT EXISTS idx_sessions_fp_time ON sessions(fingerprint, started_at);
CREATE INDEX IF NOT EXISTS idx_sessions_time    ON sessions(started_at);

CREATE TABLE IF NOT EXISTS fidelity_events (
  id         INTEGER PRIMARY KEY,
  session_id TEXT,
  profile_id INTEGER NOT NULL,
  kind       TEXT NOT NULL,
  signal     TEXT,
  score      REAL,
  weight     REAL,
  detail     TEXT,
  at         INTEGER NOT NULL,
  FOREIGN KEY (session_id) REFERENCES sessions(id),
  FOREIGN KEY (profile_id) REFERENCES profiles(id)
);
CREATE INDEX IF NOT EXISTS idx_fidelity_profile_time ON fidelity_events(profile_id, at);

CREATE TABLE IF NOT EXISTS synthesis_events (
  id             INTEGER PRIMARY KEY,
  fingerprint    TEXT NOT NULL,
  backend        TEXT NOT NULL,
  outcome        TEXT NOT NULL,
  new_profile_id INTEGER,
  duration_ms    INTEGER,
  cost_usd_est   REAL,
  error_detail   TEXT,
  at             INTEGER NOT NULL,
  FOREIGN KEY (new_profile_id) REFERENCES profiles(id)
);
CREATE INDEX IF NOT EXISTS idx_synthesis_fp_time ON synthesis_events(fingerprint, at);

CREATE TABLE IF NOT EXISTS gain_daily (
  date              TEXT PRIMARY KEY,
  runs              INTEGER NOT NULL,
  raw_bytes         INTEGER NOT NULL,
  compressed_bytes  INTEGER NOT NULL,
  tokens_saved_est  INTEGER NOT NULL,
  usd_saved_est     REAL NOT NULL,
  synthesis_cost_usd REAL NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS trust_keys (
  key_id     TEXT PRIMARY KEY,
  public_key BLOB NOT NULL,
  label      TEXT NOT NULL,
  added_at   INTEGER NOT NULL,
  revoked_at INTEGER
);

CREATE TABLE IF NOT EXISTS profile_import_events (
  id            INTEGER PRIMARY KEY,
  profile_id    INTEGER,
  source_uri    TEXT,
  signer_key_id TEXT,
  outcome       TEXT NOT NULL,
  at            INTEGER NOT NULL,
  FOREIGN KEY (profile_id)    REFERENCES profiles(id),
  FOREIGN KEY (signer_key_id) REFERENCES trust_keys(key_id)
);

CREATE TABLE IF NOT EXISTS agent_registry (
  agent            TEXT PRIMARY KEY,
  hook_path        TEXT NOT NULL,
  hook_sha256      TEXT NOT NULL,
  installed_at     INTEGER NOT NULL,
  original_backup  TEXT
);
