-- One-time upgrade of the live database to the columns schema.sql now creates (run once, 2026-10-30):
--   npx wrangler d1 execute deskmonk --remote --file=migrate.sql
ALTER TABLE events ADD COLUMN country TEXT;
ALTER TABLE events ADD COLUMN plan TEXT;
ALTER TABLE events ADD COLUMN channel TEXT;
ALTER TABLE events ADD COLUMN detail TEXT;
ALTER TABLE subscriptions ADD COLUMN plan TEXT;
ALTER TABLE subscriptions ADD COLUMN created_at INTEGER;
ALTER TABLE subscriptions ADD COLUMN email TEXT;
-- 2026-10-30, second run (the /buy → /welcome order reference):
--   npx wrangler d1 execute deskmonk --remote --command "ALTER TABLE subscriptions ADD COLUMN ref TEXT; CREATE INDEX IF NOT EXISTS subscriptions_ref ON subscriptions(ref);"
