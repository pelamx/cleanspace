CREATE TABLE IF NOT EXISTS events(
    id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, install_id TEXT NOT NULL, version TEXT NOT NULL,
    os TEXT NOT NULL, client TEXT NOT NULL, kind TEXT NOT NULL, files INTEGER NOT NULL, bytes INTEGER NOT NULL,
    duplicate_bytes INTEGER NOT NULL, junk_bytes INTEGER NOT NULL, old_download_bytes INTEGER NOT NULL, large_bytes INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS events_install ON events(install_id, ts);
CREATE INDEX IF NOT EXISTS events_ts ON events(ts);
CREATE TABLE IF NOT EXISTS hardware(install_id TEXT PRIMARY KEY, ts INTEGER NOT NULL, specs TEXT NOT NULL);
-- Pro subscriptions from Paddle webhooks; a license key works on up to 3 installations (activations).
CREATE TABLE IF NOT EXISTS subscriptions(
    id TEXT PRIMARY KEY, customer_id TEXT NOT NULL, status TEXT NOT NULL, period_end INTEGER NOT NULL,
    license_key TEXT NOT NULL UNIQUE, install_id TEXT, txn TEXT, event_at TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS subscriptions_install ON subscriptions(install_id);
CREATE INDEX IF NOT EXISTS subscriptions_txn ON subscriptions(txn);
CREATE TABLE IF NOT EXISTS activations(license_key TEXT NOT NULL, install_id TEXT NOT NULL, ts INTEGER NOT NULL, PRIMARY KEY(license_key, install_id));
