package store

// schema defines the SQLite database schema for event storage.
// Auto-migrated on startup. WAL mode for concurrent read performance.
// secure_delete zeroes freed pages (rows echo untrusted agent file paths);
// max_page_count caps the DB at 10000 pages (~40 MB at the default 4 KiB
// page size) so an agent flooding events cannot fill the host disk.
const schema = `
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
PRAGMA secure_delete = ON;
PRAGMA max_page_count = 10000;

CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL,
    file TEXT NOT NULL,
    op TEXT NOT NULL,
    size INTEGER DEFAULT 0,
    timestamp TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_events_timestamp ON events(timestamp);
CREATE INDEX IF NOT EXISTS idx_events_agent ON events(agent);
`
