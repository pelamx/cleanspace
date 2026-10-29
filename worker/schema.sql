CREATE TABLE IF NOT EXISTS events(
    id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, install_id TEXT NOT NULL, version TEXT NOT NULL,
    os TEXT NOT NULL, client TEXT NOT NULL, kind TEXT NOT NULL, files INTEGER NOT NULL, bytes INTEGER NOT NULL,
    duplicate_bytes INTEGER NOT NULL, junk_bytes INTEGER NOT NULL, old_download_bytes INTEGER NOT NULL, large_bytes INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS events_install ON events(install_id, ts);
CREATE INDEX IF NOT EXISTS events_ts ON events(ts);
CREATE TABLE IF NOT EXISTS hardware(install_id TEXT PRIMARY KEY, ts INTEGER NOT NULL, specs TEXT NOT NULL);
