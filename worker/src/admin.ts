//! The operator's panel at /admin (HTTP Basic auth: user "admin", password = ADMIN_TOKEN secret).
//! GET  /admin                 analytics + subscriptions; ?q= searches subscriptions
//! GET  /admin/user/<id>       one installation: usage, hardware, licenses, every event
//! GET  /admin/export.csv      ?what=installs|subscriptions
//! POST /admin/mine            mark an installation as the operator's own (left out of every count)
//! POST /admin/issue           a Pro license nobody paid for (press, support, testers)
//! POST /admin/{extend,devices,rotate,end,delete}   manage one subscription
//! Money moves only in Paddle (refunds, cancellations); here the license side is changed.

import type { Env } from "./index";
import { USD } from "./i18n";
import { MAX_DEVICES, newKey } from "./license";
import { COUNTRY_POINTS, LAND_ROWS, MAP_NORTH, MAP_STEP } from "./world";

const DAY = 86400;
/** The operator's day: Turkey time (UTC+3, no daylight saving). */
const TZ = 3 * 3600;
/** USD list prices (i18n.ts); Paddle's own figures are in its dashboard. */
const PRICE = USD;
const OSES = ["macos", "windows", "linux"];
/** Events minus the operator's own installations. */
const EV = "ev AS (SELECT * FROM events WHERE install_id NOT IN (SELECT install_id FROM ignored_installs))";
const KIND_LABEL: Record<string, string> = {
  Bilgisayar: "Computer", Sistem: "System", "İşlemci": "Processor", Bellek: "Memory", "Ekran kartı": "Graphics",
  Ekran: "Display", Depolama: "Storage", "Disk alanı": "Disk space", Pil: "Battery",
};

type Row = Record<string, any>;
interface Notice { message?: string; error?: string }

// ---------- entry ----------

export async function handleAdmin(req: Request, env: Env, path: string): Promise<Response | null> {
  if (path !== "/admin" && !path.startsWith("/admin/")) return null;
  const token = env.ADMIN_TOKEN ?? "";
  if (token.length < 12) return new Response("ADMIN_TOKEN must be set (at least 12 characters)", { status: 500 });
  if (!(await sameSecret(req.headers.get("Authorization") ?? "", `Basic ${btoa(`admin:${token}`)}`))) {
    return new Response(null, { status: 401, headers: { "WWW-Authenticate": 'Basic realm="deskMonk admin"' } });
  }
  const url = new URL(req.url);
  if (req.method === "GET") {
    if (path === "/admin") return html(await dashboard(env, url.searchParams.get("q")?.trim() ?? "", {}));
    if (path.startsWith("/admin/user/")) return html(await userPage(env.DB, decodeURIComponent(path.slice("/admin/user/".length))));
    if (path === "/admin/export.csv") return exportCsv(env.DB, url.searchParams.get("what") ?? "");
    return new Response("Not found", { status: 404 });
  }
  if (req.method !== "POST") return new Response("Not found", { status: 404 });
  // A browser that knows the password sends it with any request, also a form posted from another site.
  if (!sameOrigin(req)) return new Response("Cross-site request refused", { status: 403 });
  const form = await req.formData();
  const action = path.slice("/admin/".length);
  if (action === "mine") return mine(env.DB, form);
  if (action === "issue") return issue(env, form);
  if (["extend", "devices", "rotate", "end", "delete"].includes(action)) return manage(env, form, action);
  return new Response("Not found", { status: 404 });
}

/** Compares hashes, so neither the length nor the first wrong character leaks through timing. */
async function sameSecret(a: string, b: string): Promise<boolean> {
  const hash = async (s: string) => new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s)));
  return crypto.subtle.timingSafeEqual(await hash(a), await hash(b));
}

function sameOrigin(req: Request): boolean {
  const origin = req.headers.get("Origin");
  if (origin === null) return true;
  try {
    return new URL(origin).origin === new URL(req.url).origin;
  } catch {
    return false;
  }
}

function html(body: string): Response {
  return new Response(body, { headers: { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-store" } });
}

const now = () => Math.floor(Date.now() / 1000);

/** Written after the change it describes; never fails the change. */
async function audit(db: D1Database, action: string, ref: string | null, detail: string) {
  try {
    await db.prepare("INSERT INTO admin_log(ts, action, ref, detail) VALUES (?1, ?2, ?3, ?4)").bind(now(), action, ref, detail).run();
  } catch (e) {
    console.error("admin log", e);
  }
}

// ---------- actions ----------

async function mine(db: D1Database, form: FormData): Promise<Response> {
  const id = String(form.get("install") ?? "").trim();
  if (/^[A-Za-z0-9._-]{1,64}$/.test(id)) {
    if (form.get("mine") === "1") await db.prepare("INSERT OR IGNORE INTO ignored_installs(install_id, ts) VALUES (?1, ?2)").bind(id, now()).run();
    else await db.prepare("DELETE FROM ignored_installs WHERE install_id = ?1").bind(id).run();
  }
  const back = String(form.get("back") ?? "");
  return new Response(null, { status: 303, headers: { Location: back.startsWith("/admin") ? back : "/admin#users" } });
}

async function issue(env: Env, form: FormData): Promise<Response> {
  const email = String(form.get("email") ?? "").trim();
  if (email.length > 200 || (email !== "" && !/^[^\s@]+@[^\s@]+$/.test(email))) return html(await dashboard(env, "", { error: "That email address does not look right." }));
  const plan = form.get("plan") === "monthly" ? "monthly" : "yearly";
  const rawDays = String(form.get("days") ?? "").trim();
  const days = rawDays === "" ? (plan === "yearly" ? 365 : 31) : Number(rawDays);
  if (!Number.isInteger(days) || days < 1 || days > 3650) return html(await dashboard(env, "", { error: "Days must be between 1 and 3650." }));
  const id = `manual_${[...crypto.getRandomValues(new Uint8Array(5))].map((b) => b.toString(16).padStart(2, "0")).join("")}`;
  const key = newKey();
  const end = now() + days * DAY;
  await env.DB.prepare(
    `INSERT INTO subscriptions(id, customer_id, status, period_end, license_key, install_id, txn, event_at, plan, created_at, email)
     VALUES (?1, 'manual', 'active', ?2, ?3, NULL, NULL, ?4, ?5, ?6, ?7)`,
  )
    .bind(id, end, key, new Date().toISOString(), plan, now(), email || null)
    .run();
  await audit(env.DB, "issue", id, `${plan}, ${days} days, ends ${date(end)}${email ? `, ${email}` : ""}`);
  return html(await dashboard(env, id, { message: `Issued ${id}: ${days} days, ends ${date(end)}. Key: ${key}` }));
}

async function manage(env: Env, form: FormData, action: string): Promise<Response> {
  const db = env.DB;
  const id = String(form.get("id") ?? "").trim();
  const sub = await db.prepare("SELECT * FROM subscriptions WHERE id = ?1").bind(id).first<Row>();
  if (sub === null) return html(await dashboard(env, "", { error: "That subscription no longer exists." }));
  const paddle = sub.customer_id !== "manual";
  let message: string;
  switch (action) {
    case "extend": {
      const days = Number(form.get("days"));
      if (!Number.isInteger(days) || days < 1 || days > 3650) return html(await dashboard(env, id, { error: "Days must be between 1 and 3650." }));
      // From whichever is later, so a lapsed license gets the full days.
      const end = Math.max(sub.period_end, now()) + days * DAY;
      await db.prepare("UPDATE subscriptions SET period_end = ?1, status = CASE WHEN status = 'canceled' THEN 'active' ELSE status END WHERE id = ?2").bind(end, id).run();
      await audit(db, "extend", id, `+${days} days: ${date(sub.period_end)} → ${date(end)}`);
      message = `Extended by ${days} days, now ends ${date(end)}.${paddle ? " Paddle's next renewal event sets its own date again." : ""}`;
      break;
    }
    case "devices": {
      const { meta } = await db.prepare("DELETE FROM activations WHERE license_key = ?1").bind(sub.license_key).run();
      await audit(db, "free devices", id, `freed ${meta.changes ?? 0}`);
      message = `Freed ${meta.changes ?? 0} device slots. The key can be used on ${MAX_DEVICES} computers again.`;
      break;
    }
    case "rotate": {
      const key = newKey();
      await db.batch([
        db.prepare("DELETE FROM activations WHERE license_key = ?1").bind(sub.license_key),
        db.prepare("UPDATE subscriptions SET license_key = ?1 WHERE id = ?2").bind(key, id),
      ]);
      await audit(db, "new key", id, "old key disabled");
      message = `New key ${key}. The old key no longer works; each computer has to enter the new one.`;
      break;
    }
    case "end": {
      await db.prepare("UPDATE subscriptions SET status = 'canceled', period_end = ?1 WHERE id = ?2").bind(now(), id).run();
      await audit(db, "end", id, `was ${sub.status}, paid to ${date(sub.period_end)}`);
      message = paddle
        ? "Pro ended now. Cancel or refund it in Paddle too: Paddle's next event would turn it back on, and only Paddle moves money."
        : "Pro ended now.";
      break;
    }
    default: {
      // Deleting a live Paddle subscription here would leave Paddle charging for a license that no longer exists.
      if (paddle && sub.period_end > now() && sub.status !== "canceled") {
        return html(await dashboard(env, id, { error: `${id} is still live in Paddle. Cancel it in Paddle first, or use End to stop Pro now.` }));
      }
      await db.batch([
        db.prepare("DELETE FROM activations WHERE license_key = ?1").bind(sub.license_key),
        db.prepare("DELETE FROM subscriptions WHERE id = ?1").bind(id),
      ]);
      await audit(db, "delete", id, `${sub.plan ?? ""} ${sub.status}, ${sub.email ?? sub.customer_id}`);
      return html(await dashboard(env, "", { message: `Deleted ${id}. Its key no longer works.` }));
    }
  }
  return html(await dashboard(env, id, { message }));
}

// ---------- CSV ----------

/** One CSV cell, quoted; a value a spreadsheet would run as a formula is kept as text. */
function cell(v: unknown): string {
  let s = v === null || v === undefined ? "" : String(v);
  if (/^[=+\-@\t\r]/.test(s)) s = `'${s}`;
  return `"${s.replace(/"/g, '""')}"`;
}

async function exportCsv(db: D1Database, what: string): Promise<Response> {
  let rows: Row[];
  if (what === "installs") rows = await usersQuery(db, 0, 100000);
  else if (what === "subscriptions") rows = (await subsQuery(db).all<Row>()).results;
  else return new Response("Not found", { status: 404 });
  const cols = Object.keys(rows[0] ?? { empty: "" });
  const body = [cols, ...rows.map((r) => cols.map((c) => r[c]))].map((l) => l.map(cell).join(",")).join("\r\n") + "\r\n";
  return new Response(body, {
    headers: {
      "Content-Type": "text/csv; charset=utf-8",
      "Content-Disposition": `attachment; filename="deskmonk-${what}-${date(now())}.csv"`,
      "Cache-Control": "no-store",
    },
  });
}

// ---------- queries ----------

const subsQuery = (db: D1Database) =>
  db.prepare(
    `SELECT s.id, s.customer_id, s.email, s.plan, s.status, s.period_end, s.created_at, s.license_key,
            s.install_id, s.txn, s.ref, s.event_at, (SELECT COUNT(*) FROM activations a WHERE a.license_key = s.license_key) AS devices,
            (SELECT group_concat(a.install_id) FROM activations a WHERE a.license_key = s.license_key) AS device_ids
       FROM subscriptions s
      ORDER BY COALESCE(s.created_at, 0) DESC LIMIT 5000`,
  );

/** One row per installation seen since `since`, newest first. */
async function usersQuery(db: D1Database, since: number, limit: number): Promise<Row[]> {
  const latest = (col: string) =>
    `(SELECT x.${col} FROM events x WHERE x.install_id = e.install_id AND x.${col} IS NOT NULL ORDER BY x.ts DESC LIMIT 1)`;
  const { results } = await db
    .prepare(
      `SELECT e.install_id AS id, ${latest("os")} AS os, ${latest("country")} AS country, ${latest("version")} AS version,
              ${latest("plan")} AS plan, ${latest("channel")} AS channel, group_concat(DISTINCT e.client) AS client,
              (SELECT json_extract(s.value, '$.name') FROM hardware h, json_each(h.specs) s
                WHERE h.install_id = e.install_id AND json_extract(s.value, '$.kind') = 'İşlemci' LIMIT 1) AS cpu,
              MIN(e.ts) AS first_seen, MAX(e.ts) AS last_seen, SUM(e.kind = 'open') AS opens, SUM(e.kind = 'scan') AS scans,
              SUM(e.kind = 'clean') AS cleans, SUM(CASE WHEN e.kind = 'clean' THEN e.bytes ELSE 0 END) AS freed,
              SUM(e.kind = 'vault') AS vault, SUM(e.kind = 'focus') AS focus, SUM(e.kind = 'pro') AS pro_views,
              EXISTS (SELECT 1 FROM ignored_installs i WHERE i.install_id = e.install_id) AS mine
         FROM events e GROUP BY e.install_id HAVING MAX(e.ts) >= ?1 ORDER BY last_seen DESC LIMIT ?2`,
    )
    .bind(since, limit)
    .all<Row>();
  return results;
}

// ---------- dashboard ----------

async function dashboard(env: Env, query: string, notice: Notice): Promise<string> {
  const db = env.DB;
  const t = now();
  const today = Math.floor((t + TZ) / DAY) * DAY - TZ;
  const yesterday = today - DAY;
  const week = t - 7 * DAY;
  const month = t - 30 * DAY;
  const first = `f AS (SELECT install_id, MAX(os) AS os, MIN(ts) AS first, MAX(ts) AS last FROM ev GROUP BY install_id)`;
  const local = (col: string) => `date(${col} + ${TZ}, 'unixepoch')`;
  const results = await db.batch([
    db.prepare(`WITH ${EV}, ${first} SELECT os, COUNT(*) AS total, SUM(first >= ?1) AS today, SUM(first >= ?2 AND first < ?1) AS yesterday,
                  SUM(first <= ?3) AS day_base, SUM(first <= ?3 AND last - first >= ${DAY}) AS day_back,
                  SUM(first <= ?4) AS week_base, SUM(first <= ?4 AND last - first >= ${7 * DAY}) AS week_back,
                  SUM(last >= ?5 AND last < ?4) AS lost FROM f GROUP BY os`).bind(today, yesterday, t - DAY, week, month),
    db.prepare(`WITH ${EV} SELECT os, COUNT(DISTINCT CASE WHEN ts >= ?1 THEN install_id END) AS today,
                  COUNT(DISTINCT CASE WHEN ts >= ?2 AND ts < ?1 THEN install_id END) AS yesterday,
                  COUNT(DISTINCT CASE WHEN ts >= ?3 THEN install_id END) AS week, COUNT(DISTINCT install_id) AS month,
                  SUM(CASE WHEN kind = 'clean' AND ts >= ?1 THEN bytes ELSE 0 END) AS cleaned_today,
                  SUM(CASE WHEN kind = 'clean' AND ts >= ?2 AND ts < ?1 THEN bytes ELSE 0 END) AS cleaned_yesterday
                FROM ev WHERE ts >= ?4 GROUP BY os`).bind(today, yesterday, week, month),
    db.prepare(`WITH ${EV}, ${first} SELECT ${local("first")} AS d, COUNT(*) AS n FROM f WHERE first >= ?1 GROUP BY d`).bind(today - 29 * DAY),
    db.prepare(`SELECT e.os, COUNT(DISTINCT e.install_id) AS total, COUNT(DISTINCT CASE WHEN e.ts >= ?1 THEN e.install_id END) AS week
                FROM events e JOIN ignored_installs USING (install_id) GROUP BY e.os`).bind(week),
    // Plan and store of each installation's latest event (SQLite takes the bare columns from the MAX(ts) row).
    db.prepare(`WITH ${EV}, last AS (SELECT install_id, plan, channel, MAX(ts) FROM ev WHERE ts >= ?1 AND plan IS NOT NULL GROUP BY install_id)
                SELECT channel, plan, COUNT(*) AS n FROM last GROUP BY channel, plan`).bind(month),
    db.prepare(`WITH ${EV} SELECT kind, COALESCE(detail, '') AS detail, COUNT(*) AS n, COUNT(DISTINCT install_id) AS users,
                  SUM(ts >= ?2) AS week, SUM(ts >= ?3) AS today, SUM(ts >= ?4 AND ts < ?3) AS yesterday, SUM(bytes) AS bytes
                FROM ev WHERE ts >= ?1 GROUP BY kind, detail`).bind(month, week, today, yesterday),
    db.prepare(`WITH ${EV} SELECT COALESCE(country, '?') AS country, COUNT(DISTINCT CASE WHEN ts >= ?1 THEN install_id END) AS today,
                  COUNT(DISTINCT CASE WHEN ts >= ?2 THEN install_id END) AS week, COUNT(DISTINCT install_id) AS month,
                  COUNT(DISTINCT CASE WHEN plan = 'pro' THEN install_id END) AS pro
                FROM ev WHERE ts >= ?3 GROUP BY 1 ORDER BY month DESC, week DESC`).bind(today, week, month),
    db.prepare(`WITH ${EV} SELECT SUM(CASE WHEN kind = 'scan' THEN bytes END) AS found, SUM(CASE WHEN kind = 'clean' THEN bytes END) AS cleaned,
                  SUM(CASE WHEN kind = 'clean' THEN files END) AS files, SUM(kind = 'scan') AS scans,
                  SUM(CASE WHEN kind = 'scan' THEN duplicate_bytes END) AS dup, SUM(CASE WHEN kind = 'scan' THEN junk_bytes END) AS junk,
                  SUM(CASE WHEN kind = 'scan' THEN old_download_bytes END) AS old, SUM(CASE WHEN kind = 'scan' THEN large_bytes END) AS large
                FROM ev`),
    db.prepare(`WITH ${EV} SELECT version, os, COUNT(DISTINCT install_id) AS n FROM ev WHERE ts >= ?1 GROUP BY version, os`).bind(month),
    db.prepare(`WITH ${EV} SELECT client, COUNT(DISTINCT install_id) AS n FROM ev WHERE ts >= ?1 GROUP BY client ORDER BY n DESC`).bind(month),
    db.prepare(`SELECT json_extract(s.value, '$.kind') AS kind, json_extract(s.value, '$.brand') AS brand, COUNT(DISTINCT h.install_id) AS n
                FROM hardware h, json_each(h.specs) s
                WHERE json_extract(s.value, '$.brand') != '' AND h.install_id NOT IN (SELECT install_id FROM ignored_installs)
                GROUP BY 1, 2 ORDER BY 1, 3 DESC`),
    db.prepare(`WITH ${EV} SELECT ${local("ts")} AS d, COUNT(DISTINCT install_id) AS active, SUM(kind = 'scan') AS scans, SUM(kind = 'clean') AS cleans,
                  SUM(CASE WHEN kind = 'clean' THEN bytes ELSE 0 END) AS freed, SUM(kind = 'vault') AS vault, SUM(kind = 'focus') AS focus,
                  SUM(kind = 'pro') AS pro_views FROM ev WHERE ts >= ?1 GROUP BY d ORDER BY d DESC`).bind(today - 29 * DAY),
    subsQuery(db),
    db.prepare("SELECT ts, action, ref, detail FROM admin_log ORDER BY id DESC LIMIT 60"),
  ]);
  const [installs, active, series, mineOs, plans, features, countries, space, versions, clients, brands, daily, subsAll, log] =
    results.map((r) => (r.results ?? []) as Row[]);
  const users = await usersQuery(db, month, 500);

  const sum = (rows: Row[], f: string) => rows.reduce((a, r) => a + (Number(r[f]) || 0), 0);
  const byOs = (rows: Row[], f: string) => Object.fromEntries(rows.map((r) => [r.os, Number(r[f]) || 0]));
  const feature = (kind: string, detail = "") => features.find((r) => r.kind === kind && r.detail === detail) ?? {};
  const featureAll = (kind: string) => features.filter((r) => r.kind === kind);

  // Subscriptions
  const live = (s: Row) => s.period_end > t && s.status !== "canceled";
  const paddleSubs = subsAll.filter((s) => s.customer_id !== "manual");
  const livePaddle = paddleSubs.filter(live);
  const liveManual = subsAll.filter((s) => s.customer_id === "manual" && live(s));
  const appstorePro = plans.filter((p) => p.channel === "appstore" && p.plan === "pro").reduce((a, p) => a + p.n, 0);
  const proInstalls = plans.filter((p) => p.plan === "pro").reduce((a, p) => a + p.n, 0);
  const freeInstalls = plans.filter((p) => p.plan === "free").reduce((a, p) => a + p.n, 0);
  const monthly = livePaddle.filter((s) => s.plan !== "yearly").length;
  const yearly = livePaddle.filter((s) => s.plan === "yearly").length;
  const mrr = monthly * PRICE.monthly + (yearly * PRICE.yearly) / 12;
  const ending = subsAll.filter((s) => live(s) && s.period_end <= t + 7 * DAY);
  const newPaddle = (from: number, to = Infinity) => paddleSubs.filter((s) => (s.created_at ?? 0) >= from && (s.created_at ?? 0) < to).length;
  const bought = featureAll("bought");
  const appstoreToday = sum(bought, "today");
  const appstoreYesterday = sum(bought, "yesterday");

  // Users and counts
  const real = users.filter((u) => !u.mine).length;
  const own = users.length - real;
  const seriesMap = Object.fromEntries(series.map((r) => [r.d, r.n]));
  const days14 = [...Array(14)].map((_, i) => date(today + TZ - (13 - i) * DAY));
  const peak = Math.max(1, ...days14.map((d) => seriesMap[d] ?? 0));
  const bars = days14
    .map((d, i) => `<div style="height:${Math.round(((seriesMap[d] ?? 0) / peak) * 100)}%" title="${d}: ${seriesMap[d] ?? 0}${i === 13 ? " (today)" : ""}"></div>`)
    .join("");
  const todayLabel = new Date((today + TZ) * 1000).toLocaleDateString("en-GB", { weekday: "long", day: "numeric", month: "long", timeZone: "UTC" });

  const q = query.toLowerCase();
  const matches = query
    ? subsAll.filter((s) =>
        [s.id, s.customer_id, s.email, s.license_key, s.install_id, s.txn, s.ref, s.device_ids].some((v) => typeof v === "string" && v.toLowerCase().includes(q)),
      )
    : subsAll;

  const nav = [
    ["overview", "grid", "Overview", ""],
    ["analytics", "chart", "Analytics", ""],
    ["users", "users", "Users", count(real)],
    ["subscriptions", "key", "Subscriptions", ending.length ? count(ending.length, "warn") : count(livePaddle.length + liveManual.length)],
    ["revenue", "wallet", "Revenue", ""],
    ["tools", "tools", "Tools", ""],
  ]
    .map(([id, ic, label, extra]) => `<a href="#${id}" title="${label}">${icon(ic)}<span class="nav-label">${label}</span>${extra}</a>`)
    .join("");

  const prod = env.PADDLE_ENV === "production";
  const body = `
  <header class="appbar">
    <a class="brand" href="/admin"><img src="/logo.svg" alt="">deskMonk</a><span class="tag">ADMIN</span>
    <span class="spacer"></span>
    <a class="btn" href="/admin/export.csv?what=installs" title="Installations CSV">${icon("file")}<span>Installs CSV</span></a>
    <a class="btn" href="/admin/export.csv?what=subscriptions" title="Subscriptions CSV">${icon("file")}<span>Subscriptions CSV</span></a>
    <a class="btn" href="/" title="deskmonk.app">${icon("globe")}<span>Site</span></a>
    <span class="env${prod ? " prod" : ""}" title="Paddle environment">Paddle ${esc(env.PADDLE_ENV ?? "sandbox")}</span>
  </header>
  <div class="shell">
  <nav class="side" aria-label="Sections">${nav}<p class="hint">Press <kbd>/</kbd> to search subscriptions</p></nav>
  <main class="main">
  ${notice.message ? `<p class="note ok">${esc(notice.message)}</p>` : ""}
  ${notice.error ? `<p class="note err">${esc(notice.error)}</p>` : ""}

  <div class="zone" id="overview">
  <section class="card">
    <p class="label">Today <span class="aside">· ${esc(todayLabel)} (Turkey time, vs yesterday)</span></p>
    <div class="grid">
      ${tile({ icon: "userPlus", tone: "t-blue", title: "New installs", hint: "Opened deskMonk for the first time",
        value: sum(installs, "today"), extra: delta(sum(installs, "today"), sum(installs, "yesterday")), rows: osBars(byOs(installs, "today")) })}
      ${tile({ icon: "users", tone: "t-teal", title: "Active users", hint: "Used deskMonk today",
        value: sum(active, "today"), extra: delta(sum(active, "today"), sum(active, "yesterday")), rows: osBars(byOs(active, "today")) })}
      ${tile({ icon: "broom", tone: "t-green", title: "Space freed", hint: "Cleaned by everyone today",
        value: humanSize(sum(active, "cleaned_today")), extra: deltaText(humanSize(sum(active, "cleaned_yesterday"))), rows: osBars(byOs(active, "cleaned_today"), humanSize) })}
      ${tile({ icon: "cart", tone: "t-red", title: "New Pro customers", hint: "Paddle + App Store",
        value: newPaddle(today) + appstoreToday, extra: delta(newPaddle(today) + appstoreToday, newPaddle(yesterday, today) + appstoreYesterday),
        rows: bars3([["Paddle", newPaddle(today), "web"], ["App Store", appstoreToday, "store"]]) })}
    </div>
    ${places(countries)}
  </section>

  <section class="card">
    <p class="label">Overall</p>
    <div class="grid five">
      ${tile({ icon: "star", tone: "t-orange", title: "Pro now", value: livePaddle.length + liveManual.length + appstorePro,
        rows: bars3([["Paddle", livePaddle.length, "web"], ["App Store", appstorePro, "store"], ["Manual", liveManual.length, "manual"]]),
        foot: `${monthly} monthly · ${yearly} yearly on Paddle`, footIcon: "calendar" })}
      ${tile({ icon: "bars", tone: "t-violet", title: "Active this week", value: sum(active, "week"), rows: osBars(byOs(active, "week")),
        foot: `${freeInstalls} free · ${proInstalls} Pro (last 30 days)`, footIcon: "users" })}
      ${tile({ icon: "download", tone: "t-blue", title: "Installs ever seen", value: sum(installs, "total"), rows: osBars(byOs(installs, "total")),
        foot: `${sum(active, "month")} active this month`, footIcon: "chart" })}
      ${tile({ icon: "clock", tone: "t-orange", title: "Ending in 7 days", value: ending.length, cls: ending.length ? "alert" : "",
        rows: bars3([["Paddle", ending.filter((s) => s.customer_id !== "manual").length, "web"], ["Manual", ending.filter((s) => s.customer_id === "manual").length, "manual"]]),
        foot: `${paddleSubs.filter((s) => s.status === "past_due").length} payment problems (past due)`, footIcon: "card" })}
      ${tile({ icon: "laptop", tone: "t-blue", cls: "mine", title: `Your own machines <span class="info" title="Installations you marked as yours. They are left out of every other number.">ⓘ</span>`,
        value: sum(mineOs, "total"), rows: osBars(byOs(mineOs, "total")), foot: `${sum(mineOs, "week")} active this week · not counted`, footIcon: "laptop" })}
    </div>
    <p class="label" style="margin-top:20px">New installs, last 14 days</p>
    <div class="bars">${bars}</div>
    <div class="bars-axis"><span>14 days ago</span><span>today</span></div>
  </section>
  </div>

  <div class="zone" id="analytics">
  <section class="card">
    <p class="label">Features, last 30 days</p>
    <div class="scroll"><table>
      <tr><th>Feature</th><th class="num">Uses</th><th class="num">Users</th><th class="num">Last 7 days</th><th class="num">Today</th><th></th></tr>
      ${[
        ["App opened", feature("open"), ""],
        ["Scan", feature("scan"), `${humanSize(feature("scan").bytes ?? 0)} found`],
        ["Clean", feature("clean"), `${humanSize(feature("clean").bytes ?? 0)} freed`],
        ["Vault: encrypt", feature("vault", "encrypt"), ""],
        ["Vault: open", feature("vault", "open"), ""],
        ["Monk mode: focus finished", feature("focus"), ""],
        ["Pro dialog opened", merge(featureAll("pro")), ""],
        ["App Store purchase", merge(bought), ""],
      ]
        .map(([label, r, extra]: any) => `<tr><td>${label}</td><td class="num">${r.n ?? 0}</td><td class="num">${r.users ?? 0}</td>
          <td class="num">${r.week ?? 0}</td><td class="num">${r.today ?? 0}</td><td class="muted">${extra}</td></tr>`)
        .join("")}
    </table></div>
    <p class="muted foot">Vault, Monk mode, Pro dialog and plan are reported by deskMonk 0.4 and later. Users = installations.</p>
  </section>

  <section class="card">
    <p class="label">Upgrade funnel, last 30 days</p>
    <div class="scroll"><table>
      <tr><th>What opened the Pro dialog</th><th class="num">Times</th><th class="num">Installs</th></tr>
      ${[
        ["⭐ Get Pro button", "button"], ["Daily clean limit (2 GB)", "clean_limit"], ["Vault limit (3 a day / 200 MB)", "vault_limit"], ["App lock limit (1 app)", "applock_limit"],
      ]
        .map(([label, d]) => { const r = feature("pro", d); return `<tr><td>${label}</td><td class="num">${r.n ?? 0}</td><td class="num">${r.users ?? 0}</td></tr>`; })
        .join("")}
      <tr class="total"><td>Bought Pro (Paddle + App Store)</td><td class="num">${newPaddle(month) + sum(bought, "n")}</td>
        <td class="num">${ratio(newPaddle(month) + sum(bought, "n"), sum(featureAll("pro"), "users"))} of installs that saw the dialog</td></tr>
    </table></div>
  </section>

  <section class="card">
    <p class="label">Growth &amp; retention</p>
    <div class="scroll"><table>
      <tr><th>Platform</th><th class="num">Installs</th><th>Came back next day</th><th>Still using after a week</th><th class="num">Lost</th></tr>
      ${[...OSES.map((os) => installs.find((r) => r.os === os) ?? { os }), totalRow(installs)]
        .map((r) => `<tr${r.os === "all" ? ' class="total"' : ""}><td>${osName(r.os)}</td><td class="num">${r.total ?? 0}</td>
          <td>${ratio(r.day_back ?? 0, r.day_base ?? 0)}</td><td>${ratio(r.week_back ?? 0, r.week_base ?? 0)}</td><td class="num">${r.lost ?? 0}</td></tr>`)
        .join("")}
    </table></div>
    <p class="muted foot"><b>Came back / still using</b>: of the installs old enough to tell, how many were seen again at least a day / a week after their first event.
      <b>Lost</b>: used in the last 30 days, but not in the last 7.</p>

    <p class="label" style="margin-top:22px">Versions in use, last 30 days</p>
    <div class="scroll"><table>
      <tr><th>Version</th>${OSES.map((os) => `<th class="num">${osName(os)}</th>`).join("")}<th class="num">Total</th></tr>
      ${[...new Set(versions.map((v) => v.version))].sort(compareVersions)
        .map((v) => {
          const n = (os: string) => versions.filter((r) => r.version === v && r.os === os).reduce((a, r) => a + r.n, 0);
          return `<tr><td>${esc(v)}</td>${OSES.map((os) => `<td class="num">${n(os) || '<span class="muted">0</span>'}</td>`).join("")}
            <td class="num"><b>${versions.filter((r) => r.version === v).reduce((a, r) => a + r.n, 0)}</b></td></tr>`;
        })
        .join("") || `<tr><td colspan="5" class="muted">No installs this month.</td></tr>`}
    </table></div>
    <p class="muted foot">Interface: ${clients.map((c) => `${esc(c.client === "tui" ? "terminal" : c.client === "gui" ? "desktop" : c.client)} <b>${c.n}</b>`).join(" · ") || "—"}
      · Store: web ${sum(plans.filter((p) => p.channel === "web"), "n")} · App Store ${sum(plans.filter((p) => p.channel === "appstore"), "n")}</p>
  </section>

  <section class="card">
    <p class="label">Countries, last 30 days</p>
    <div class="scroll"><table>
      <tr><th>Country</th><th class="num">Installs</th><th class="num">Active 7 days</th><th class="num">Today</th><th class="num">Pro</th><th></th></tr>
      ${countries
        .map((c) => `<tr><td>${flag(c.country)} ${esc(countryName(c.country))}</td><td class="num">${c.month}</td><td class="num">${c.week}</td>
          <td class="num">${c.today}</td><td class="num">${c.pro}</td><td style="width:35%"><span class="track"><i style="width:${Math.round((c.month / Math.max(1, countries[0]?.month ?? 1)) * 100)}%"></i></span></td></tr>`)
        .join("") || `<tr><td colspan="6" class="muted">No countries yet: deskMonk 0.4 and later report them.</td></tr>`}
    </table></div>
  </section>

  <section class="card">
    <p class="label">Space</p>
    <div class="grid">
      ${stat("Found by scans", humanSize(space[0]?.found ?? 0), `${space[0]?.scans ?? 0} scans`)}
      ${stat("Freed by cleaning", humanSize(space[0]?.cleaned ?? 0), `${space[0]?.files ?? 0} files`)}
      ${stat("Cleaned of what was found", space[0]?.found ? `${Math.round(((space[0]?.cleaned ?? 0) / space[0].found) * 100)}%` : "—", "all time")}
    </div>
    <p class="label" style="margin-top:20px">What scans find</p>
    ${hbars([["Duplicates", space[0]?.dup ?? 0], ["Junk files", space[0]?.junk ?? 0], ["Old downloads", space[0]?.old ?? 0], ["Large files", space[0]?.large ?? 0]])}
  </section>

  <section class="card">
    <p class="label">Hardware brands (latest known)</p>
    <div class="scroll"><table>
      <tr><th>Part</th><th>Brand</th><th class="num">Installs</th></tr>
      ${brands.map((b) => `<tr><td>${esc(KIND_LABEL[b.kind] ?? b.kind)}</td><td>${esc(b.brand)}</td><td class="num">${b.n}</td></tr>`).join("") ||
        `<tr><td colspan="3" class="muted">No hardware reported yet.</td></tr>`}
    </table></div>
  </section>

  <details class="card">
    <summary>Day by day, last 30 days</summary>
    <div class="body scroll"><table>
      <tr><th>Day</th><th class="num">New</th><th class="num">Active</th><th class="num">Scans</th><th class="num">Cleans</th><th class="num">Freed</th>
        <th class="num">Vault</th><th class="num">Focus</th><th class="num">Pro dialog</th></tr>
      ${daily
        .map((d) => `<tr><td>${d.d}</td><td class="num">${seriesMap[d.d] ?? 0}</td><td class="num">${d.active}</td><td class="num">${d.scans}</td>
          <td class="num">${d.cleans}</td><td class="num">${humanSize(d.freed)}</td><td class="num">${d.vault}</td><td class="num">${d.focus}</td>
          <td class="num">${d.pro_views}</td></tr>`)
        .join("") || `<tr><td colspan="9" class="muted">Nothing this month.</td></tr>`}
    </table></div>
  </details>
  </div>

  <details class="card zone" id="users" open>
    <summary>Users, last 30 days: ${real} real${own ? ` · ${own} yours` : ""}</summary>
    <div class="body">
      <p class="muted" style="margin-top:0">One row per installation. Press <b>This is me</b> on your own machines: they stay listed, tagged
        <span class="badge you">you</span>, but leave every count on this page.</p>
      <div class="scroll"><table>
        <tr><th>Install</th><th>Where</th><th>System</th><th>Plan</th><th>Version</th><th>First seen</th><th>Last seen</th>
          <th class="num">Opens</th><th class="num">Scans</th><th class="num">Cleans</th><th class="num">Freed</th><th class="num">Vault</th><th class="num">Focus</th><th></th></tr>
        ${users.map((u) => userRow(u, "/admin#users")).join("") || `<tr><td colspan="14" class="muted">Nobody this month.</td></tr>`}
      </table></div>
    </div>
  </details>

  <div class="zone" id="subscriptions">
  ${ending.length ? `<details class="card warnbox" open><summary>Ending in 7 days (${ending.length})</summary><div class="body scroll"><table>
      <tr><th>Subscription</th><th>Email</th><th>Plan</th><th>Status</th><th>Ends</th></tr>
      ${ending.map((s) => `<tr><td><a href="/admin?q=${encodeURIComponent(s.id)}#subscriptions"><code>${esc(s.id)}</code></a></td><td>${esc(s.email ?? "—")}</td>
        <td>${planBadge(s)}</td><td>${statusBadge(s, t)}</td><td>${date(s.period_end)}</td></tr>`).join("")}
    </table></div></details>` : ""}
  <details class="card" id="subs" open>
    <summary>Subscriptions${query ? ` matching “${esc(query)}”` : ""}: ${matches.length}</summary>
    <div class="body">
      <form method="get" action="/admin" class="row">
        <div style="flex:1 1 280px"><label class="field" for="q">Search: email, license key, subscription id, install id</label>
          <input id="q" type="text" name="q" value="${esc(query)}" placeholder="DM-XXXX-XXXX-XXXX" spellcheck="false" autocomplete="off"></div>
        <button type="submit">Search</button>${query ? `<a class="btn-link" href="/admin#subscriptions">Clear</a>` : ""}
      </form>
      ${subGroup("Paddle", "paddle", matches.filter((s) => s.customer_id !== "manual"), t, !!query)}
      ${subGroup("Issued by hand", "manual", matches.filter((s) => s.customer_id === "manual"), t, !!query)}
      <p class="muted foot">Refunds and cancellations are done in Paddle; its webhook updates this list. App Store subscriptions are Apple's and
        appear only in the counts above (Pro installs on the App Store version).</p>
    </div>
  </details>
  </div>

  <section class="card zone" id="revenue">
    <p class="label">Revenue (Paddle, list prices, USD)</p>
    <div class="grid">
      ${stat("Monthly recurring", usd(mrr), `${monthly} × ${usd(PRICE.monthly)} + ${yearly} × ${usd(PRICE.yearly)}/12`)}
      ${stat("Yearly run rate", usd(mrr * 12), "MRR × 12")}
      ${stat("Live subscriptions", String(livePaddle.length), `${paddleSubs.filter((s) => s.status === "canceled").length} canceled all time`)}
      ${stat("App Store Pro installs", String(appstorePro), "Apple pays out; see App Store Connect")}
    </div>
    <p class="label" style="margin-top:20px">By month</p>
    <div class="scroll"><table>
      <tr><th>Month</th><th class="num">New monthly</th><th class="num">New yearly</th><th class="num">Canceled</th><th class="num">New list value</th></tr>
      ${months(paddleSubs, t)}
    </table></div>
    <p class="muted foot">Paddle takes its fee and sales tax before paying out; exact amounts are in Paddle → Reports.</p>
  </section>

  <div class="zone" id="tools">
  <details class="card">
    <summary>Issue a license</summary>
    <div class="body">
      <p class="muted">For people who never go through checkout: press, support, beta testers. It makes a working key with no payment
        behind it, marked <span class="badge manual">manual</span>; Paddle never changes it.</p>
      <form method="post" action="/admin/issue" class="row">
        <div style="flex:2 1 240px"><label class="field" for="issue-email">Email (for your records)</label>
          <input id="issue-email" type="email" name="email" placeholder="name@example.com" spellcheck="false"></div>
        <div style="flex:0 1 150px"><label class="field" for="issue-plan">Plan</label>
          <select id="issue-plan" name="plan"><option value="yearly">Yearly</option><option value="monthly">Monthly</option></select></div>
        <div style="flex:0 1 140px"><label class="field" for="issue-days">Days (empty: the plan's)</label>
          <input id="issue-days" type="number" name="days" min="1" max="3650" placeholder="365"></div>
        <button type="submit">Issue</button>
      </form>
    </div>
  </details>
  <details class="card">
    <summary>History (${log.length})</summary>
    <div class="body scroll"><table>
      <tr><th>When (UTC)</th><th>Action</th><th>Subscription</th><th>Detail</th></tr>
      ${log.map((l) => `<tr><td>${dateTime(l.ts)}</td><td>${esc(l.action)}</td><td>${l.ref ? `<a href="/admin?q=${encodeURIComponent(l.ref)}#subscriptions"><code>${esc(l.ref)}</code></a>` : ""}</td>
        <td class="muted">${esc(l.detail)}</td></tr>`).join("") || `<tr><td colspan="4" class="muted">Nothing done here yet.</td></tr>`}
    </table></div>
  </details>
  </div>
  </main>
  </div>
  <script>${SHELL_SCRIPT}</script>`;
  return layout("Admin — deskMonk", body);
}

function merge(rows: Row[]): Row {
  return ["n", "users", "week", "today"].reduce((acc, k) => ({ ...acc, [k]: rows.reduce((a, r) => a + (Number(r[k]) || 0), 0) }), {} as Row);
}

function totalRow(rows: Row[]): Row {
  const keys = ["total", "day_base", "day_back", "week_base", "week_back", "lost"];
  return { os: "all", ...Object.fromEntries(keys.map((k) => [k, rows.reduce((a, r) => a + (Number(r[k]) || 0), 0)])) };
}

function subGroup(title: string, cls: string, subs: Row[], t: number, open: boolean): string {
  if (subs.length === 0) return `<div class="group ${cls}"><p class="group-head"><b>${title}</b> <span class="muted">none</span></p></div>`;
  return `<div class="group ${cls}"><p class="group-head"><b>${title}</b> <span class="muted">${subs.length}</span></p>
    <div class="scroll"><table>
      <tr><th>Subscription</th><th>Email</th><th>Plan</th><th>Status</th><th>Started</th><th>Paid to</th><th>Devices</th><th>Key</th><th></th></tr>
      ${subs
        .map((s) => `<tr><td><code>${esc(s.id)}</code></td><td>${esc(s.email ?? "—")}</td><td>${planBadge(s)}</td><td>${statusBadge(s, t)}</td>
          <td>${s.created_at ? date(s.created_at) : "—"}</td><td>${date(s.period_end)}</td>
          <td>${s.devices}/${MAX_DEVICES} ${(s.device_ids ?? "").split(",").filter(Boolean).map((d: string) => `<a href="/admin/user/${encodeURIComponent(d)}" title="${esc(d)}">${esc(d.slice(0, 8))}</a>`).join(" ")}</td>
          <td><code class="k">${esc(s.license_key)}</code></td>
          <td><details class="act"${open ? " open" : ""}><summary>Manage</summary>${actions(s)}</details></td></tr>`)
        .join("")}
    </table></div></div>`;
}

function actions(s: Row): string {
  const id = `<input type="hidden" name="id" value="${esc(s.id)}">`;
  const confirm = (msg: string) => `onsubmit="return confirm('${msg}')"`;
  return `<div class="acts">
    <form method="post" action="/admin/extend" class="inline">${id}<input type="number" name="days" min="1" max="3650" value="30" aria-label="Days"><button>Extend days</button></form>
    <form method="post" action="/admin/devices" class="inline" ${confirm("Free all device slots of this key?")}>${id}<button class="quiet">Free devices</button></form>
    <form method="post" action="/admin/rotate" class="inline" ${confirm("Make a new key? The old key stops working on every computer.")}>${id}<button class="quiet">New key</button></form>
    <form method="post" action="/admin/end" class="inline" ${confirm("End Pro now for this subscription?")}>${id}<button class="quiet">End now</button></form>
    <form method="post" action="/admin/delete" class="inline" ${confirm("Delete this subscription and its key for good?")}>${id}<button class="danger">Delete</button></form>
  </div>`;
}

function months(subs: Row[], t: number): string {
  const rows = [...Array(6)].map((_, i) => {
    const d = new Date(t * 1000);
    d.setUTCDate(1);
    d.setUTCMonth(d.getUTCMonth() - i);
    const ym = d.toISOString().slice(0, 7);
    const made = subs.filter((s) => s.created_at && date(s.created_at).startsWith(ym));
    const m = made.filter((s) => s.plan !== "yearly").length;
    const y = made.filter((s) => s.plan === "yearly").length;
    const canceled = subs.filter((s) => s.status === "canceled" && String(s.event_at ?? "").startsWith(ym)).length;
    return `<tr><td>${ym}</td><td class="num">${m}</td><td class="num">${y}</td><td class="num">${canceled}</td><td class="num">${usd(m * PRICE.monthly + y * PRICE.yearly)}</td></tr>`;
  });
  return rows.join("");
}

function userRow(u: Row, back: string): string {
  const tags = [u.mine ? `<span class="badge you">you</span>` : "", u.channel === "appstore" ? `<span class="badge store">App Store</span>` : ""].join(" ");
  return `<tr${u.mine ? ' class="mine"' : ""}><td><a href="/admin/user/${encodeURIComponent(u.id)}"><code>${esc(String(u.id).slice(0, 8))}</code></a> ${tags}</td>
    <td>${u.country ? `${flag(u.country)} ${esc(u.country)}` : '<span class="muted">—</span>'}</td>
    <td>${osIcon(u.os)} ${esc(u.cpu ?? osName(u.os))}${u.client && u.client !== "gui" ? ` <span class="muted">(${u.client === "tui" ? "terminal" : esc(u.client)})</span>` : ""}</td>
    <td>${u.plan === "pro" ? '<span class="badge on">Pro</span>' : u.plan === "free" ? '<span class="badge">Free</span>' : '<span class="muted">—</span>'}</td>
    <td>${esc(u.version ?? "")}</td><td>${date(u.first_seen)}</td><td>${ago(u.last_seen)}</td>
    <td class="num">${u.opens}</td><td class="num">${u.scans}</td><td class="num">${u.cleans}</td><td class="num">${humanSize(u.freed)}</td>
    <td class="num">${u.vault}</td><td class="num">${u.focus}</td>
    <td><form method="post" action="/admin/mine" class="inline"><input type="hidden" name="install" value="${esc(u.id)}">
      <input type="hidden" name="mine" value="${u.mine ? 0 : 1}"><input type="hidden" name="back" value="${esc(back)}">
      <button class="quiet">${u.mine ? "Not me" : "This is me"}</button></form></td></tr>`;
}

// ---------- one installation ----------

async function userPage(db: D1Database, id: string): Promise<string> {
  const [summary, events, subs] = (
    await db.batch([
      db.prepare(`SELECT SUM(kind = 'open') AS opens, SUM(kind = 'scan') AS scans, SUM(kind = 'clean') AS cleans,
                    SUM(CASE WHEN kind = 'clean' THEN bytes END) AS freed, SUM(CASE WHEN kind = 'scan' THEN bytes END) AS found,
                    SUM(kind = 'vault') AS vault, SUM(kind = 'focus') AS focus, SUM(kind = 'pro') AS pro_views,
                    MIN(ts) AS first_seen, MAX(ts) AS last_seen FROM events WHERE install_id = ?1`).bind(id),
      db.prepare(`SELECT ts, kind, detail, client, os, version, country, plan, channel, files, bytes FROM events WHERE install_id = ?1 ORDER BY ts DESC LIMIT 1000`).bind(id),
      db.prepare(`SELECT s.id, s.plan, s.status, s.period_end, s.license_key, s.customer_id FROM subscriptions s
                  WHERE s.install_id = ?1 OR s.license_key IN (SELECT license_key FROM activations WHERE install_id = ?1)`).bind(id),
    ])
  ).map((r) => (r.results ?? []) as Row[]);
  const u = (await usersQuery(db, 0, 100000)).find((r) => r.id === id);
  const s = summary[0] ?? {};
  const t = now();
  let body = `<header class="appbar"><a class="brand" href="/admin"><img src="/logo.svg" alt="">deskMonk</a><span class="tag">ADMIN</span></header>
  <main class="main solo">
  <p><a href="/admin#users">← All users</a></p>
  <h1 class="title">Installation <code>${esc(id)}</code> ${u?.mine ? '<span class="badge you">you</span>' : ""}</h1>
  <p class="muted">${u ? `${u.country ? `${flag(u.country)} ${esc(countryName(u.country))} · ` : ""}${osName(u.os)} · ${esc(u.version ?? "")} ·
    ${u.plan === "pro" ? "Pro" : "Free"}${u.channel === "appstore" ? " (App Store)" : ""} · first seen ${date(u.first_seen)} · last seen ${ago(u.last_seen)}` : "No events from this installation."}</p>
  ${u ? `<form method="post" action="/admin/mine" class="inline"><input type="hidden" name="install" value="${esc(id)}"><input type="hidden" name="mine" value="${u.mine ? 0 : 1}">
    <input type="hidden" name="back" value="/admin/user/${esc(encodeURIComponent(id))}"><button class="quiet">${u.mine ? "Not me" : "This is me"}</button></form>` : ""}
  <section class="card" style="margin-top:16px"><div class="grid">
    ${stat("App opened", String(s.opens ?? 0))}${stat("Scans", String(s.scans ?? 0), `${humanSize(s.found ?? 0)} found`)}
    ${stat("Cleans", String(s.cleans ?? 0), `${humanSize(s.freed ?? 0)} freed`)}${stat("Vault", String(s.vault ?? 0))}
    ${stat("Focus sessions", String(s.focus ?? 0))}${stat("Pro dialog", String(s.pro_views ?? 0))}
  </div></section>`;
  if (subs.length) {
    body += `<section class="card"><p class="label">Licenses on this installation</p><div class="scroll"><table>
      <tr><th>Subscription</th><th>Plan</th><th>Status</th><th>Paid to</th><th>Key</th></tr>
      ${subs.map((x) => `<tr><td><a href="/admin?q=${encodeURIComponent(x.id)}#subscriptions"><code>${esc(x.id)}</code></a></td><td>${planBadge(x)}</td>
        <td>${statusBadge(x, t)}</td><td>${date(x.period_end)}</td><td><code class="k">${esc(x.license_key)}</code></td></tr>`).join("")}
    </table></div></section>`;
  }
  const hw = await db.prepare("SELECT ts, specs FROM hardware WHERE install_id = ?1").bind(id).first<{ ts: number; specs: string }>();
  body += hw ? hardwareHtml(dateTime(hw.ts), hw.specs) : `<section class="card"><p class="muted">No hardware reported yet.</p></section>`;
  body += `<section class="card"><p class="label">Events (${events.length}${events.length === 1000 ? ", newest" : ""})</p><div class="scroll"><table>
    <tr><th>When (UTC)</th><th>Event</th><th>Detail</th><th>Interface</th><th>Version</th><th>Plan</th><th>Where</th><th class="num">Files</th><th class="num">Size</th></tr>
    ${events.map((e) => `<tr><td>${dateTime(e.ts)}</td><td>${esc(e.kind)}</td><td>${esc(e.detail ?? "")}</td><td>${esc(e.client)}</td><td>${esc(e.version)}</td>
      <td>${esc(e.plan ?? "")}</td><td>${e.country ? `${flag(e.country)} ${esc(e.country)}` : ""}</td><td class="num">${e.files || ""}</td>
      <td class="num">${e.bytes ? humanSize(e.bytes) : ""}</td></tr>`).join("")}
  </table></div></section></main>`;
  return layout(`Install ${esc(id)} — deskMonk admin`, body);
}

/** The installation's latest hardware as a terminal-style listing. */
export function hardwareHtml(ts: string, specsJson: string): string {
  let specs: Record<string, string>[] = [];
  try {
    specs = JSON.parse(specsJson);
  } catch {}
  let out = `<pre class=term><span class=dim>root@deskmonk:~$ ./hwinfo --brands  # ${esc(ts)} UTC</span>\n`;
  for (const s of specs) {
    const field = (k: string) => esc(s[k] ?? "");
    const brand = field("brand") ? `<span class=b>[${field("brand").toUpperCase()}]</span> ` : "";
    out += `<span class=k>${esc((KIND_LABEL[s.kind ?? ""] ?? s.kind ?? "").toUpperCase())}</span> ${brand}${field("name")}\n`;
    if (field("detail")) out += `<span class=dim>${" ".repeat(12)} └─ ${field("detail")}</span>\n`;
  }
  return out + "<span class=dim>root@deskmonk:~$</span> <span class=cur>█</span></pre>";
}

// ---------- small pieces ----------

export function esc(s: string): string {
  return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&#39;");
}

export function humanSize(bytes: number): string {
  const UNITS = ["B", "KB", "MB", "GB", "TB"];
  let [size, unit] = [Number(bytes) || 0, 0];
  while (size >= 1024 && unit < UNITS.length - 1) {
    size /= 1024;
    unit++;
  }
  return unit === 0 ? `${size} B` : `${size.toFixed(1)} ${UNITS[unit]}`;
}

const date = (ts: number) => new Date(ts * 1000).toISOString().slice(0, 10);
const dateTime = (ts: number) => new Date(ts * 1000).toISOString().slice(0, 16).replace("T", " ");
const usd = (n: number) => `$${Number.isInteger(n) ? n : n.toFixed(2)}`;
const count = (n: number, tone = "") => (n > 0 ? `<span class="count${tone ? ` ${tone}` : ""}">${n}</span>` : "");

function ago(ts: number): string {
  const s = now() - ts;
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))} min ago`;
  if (s < DAY) return `${Math.round(s / 3600)} h ago`;
  if (s < 30 * DAY) return `${Math.round(s / DAY)} days ago`;
  return date(ts);
}

function ratio(n: number, of: number): string {
  if (!of) return `<span class="muted">—</span>`;
  return `${Math.round((n / of) * 100)}% <span class="muted">(${n}/${of})</span>`;
}

function delta(a: number, b: number): string {
  const tone = a > b ? "up" : a < b ? "down" : "muted";
  return `<p class="d"><span class="${tone}">${a > b ? "▲" : a < b ? "▼" : "•"} ${Math.abs(a - b)}</span> vs ${b} yesterday</p>`;
}

const deltaText = (yesterday: string) => `<p class="d"><span class="muted">${yesterday} yesterday</span></p>`;

export function compareVersions(a: string, b: string): number {
  const pa = a.split(".").map(Number);
  const pb = b.split(".").map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pb[i] || 0) - (pa[i] || 0);
    if (d) return d;
  }
  return b.localeCompare(a);
}

function osName(os: string): string {
  return { macos: "macOS", windows: "Windows", linux: "Linux", all: "All" }[os] ?? esc(os ?? "?");
}

function osIcon(os: string): string {
  if (os === "windows") return WINDOWS_ICON;
  if (os === "linux") return "🐧";
  if (os === "macos") return icon("apple");
  return "•";
}

/** Bars per platform: always macOS, Windows and Linux, zeros included, so every tile reads the same way. */
function osBars(counts: Record<string, number>, fmt: (n: number) => string = String): string {
  const oses = [...OSES, ...Object.keys(counts).filter((os) => !OSES.includes(os) && counts[os] > 0)];
  const total = Math.max(1, oses.reduce((a, os) => a + (counts[os] ?? 0), 0));
  return `<div class="osbars">${oses
    .map((os) => {
      const n = counts[os] ?? 0;
      return `<div class="osrow os-${OSES.includes(os) ? os : "other"}${n ? "" : " zero"}"><span class="ic">${osIcon(os)}</span><span class="nm">${osName(os)}</span>
        <b>${fmt(n)}</b><span class="track"><i style="width:${Math.round((n / total) * 100)}%"></i></span></div>`;
    })
    .join("")}</div>`;
}

/** Bars for any small set: [label, count, color class]. */
function bars3(rows: [string, number, string][]): string {
  const total = Math.max(1, rows.reduce((a, [, n]) => a + n, 0));
  return `<div class="osbars">${rows
    .map(([label, n, cls]) => `<div class="osrow c-${cls}${n ? "" : " zero"}"><span class="ic">•</span><span class="nm">${label}</span><b>${n}</b>
      <span class="track"><i style="width:${Math.round((n / total) * 100)}%"></i></span></div>`)
    .join("")}</div>`;
}

function hbars(rows: [string, number][]): string {
  const most = Math.max(1, ...rows.map(([, n]) => n));
  return `<div class="hbars">${rows
    .map(([label, n]) => `<div class="hbar"><span>${label}</span><span class="track"><i style="width:${Math.round((n / most) * 100)}%"></i></span><b>${humanSize(n)}</b></div>`)
    .join("")}</div>`;
}

function tile(t: { icon: string; tone: string; title: string; hint?: string; value: number | string; extra?: string; rows: string; foot?: string; footIcon?: string; cls?: string }): string {
  return `<div class="stat${t.cls ? ` ${t.cls}` : ""}">
    <div class="head"><span class="ico ${t.tone}">${icon(t.icon)}</span><p class="k">${t.title}${t.hint ? `<small>${t.hint}</small>` : ""}</p></div>
    <div class="v">${t.value}</div>${t.extra ?? ""}
    ${t.rows}
    ${t.foot ? `<p class="foot-line">${icon(t.footIcon ?? "bars")}<span>${t.foot}</span></p>` : ""}
  </div>`;
}

const stat = (k: string, v: string, d = "") => `<div class="stat plain"><p class="k">${k}</p><div class="v">${v}</div>${d ? `<p class="d">${d}</p>` : ""}</div>`;

function planBadge(s: Row): string {
  const manual = s.customer_id === "manual" ? ` <span class="badge manual">manual</span>` : "";
  return `<span class="badge">${s.plan === "yearly" ? "yearly" : s.plan === "monthly" ? "monthly" : "—"}</span>${manual}`;
}

function statusBadge(s: Row, t: number): string {
  if (s.status === "canceled" || s.period_end <= t) return `<span class="badge off">ended</span>`;
  if (s.status === "past_due") return `<span class="badge warn">past due</span>`;
  if (s.status === "trialing") return `<span class="badge new">trial</span>`;
  return `<span class="badge on">active</span>`;
}

/** A country code as its flag emoji (two regional indicator letters). */
function flag(cc: string): string {
  return /^[A-Z]{2}$/.test(cc) && cc !== "XX" && cc !== "T1" ? String.fromCodePoint(...[...cc].map((c) => 0x1f1a5 + c.charCodeAt(0))) : "🏳️";
}

function countryName(cc: string): string {
  if (cc === "?" || !cc) return "Unknown";
  if (cc === "T1") return "Tor";
  try {
    return new Intl.DisplayNames(["en"], { type: "region" }).of(cc) ?? cc;
  } catch {
    return cc;
  }
}

/** Today's active users, one small card per country. */
function places(countries: Row[]): string {
  const list = countries.filter((c) => c.today > 0).sort((a, b) => b.today - a.today);
  const total = list.reduce((a, c) => a + c.today, 0);
  return `<div class="places"><div class="split">${worldMap(list)}<div>
    <p class="title">${icon("globe")}Active today by country <span>(${total})</span></p>
    ${list.length ? `<div class="list">${list.map((c) => `<div class="place" title="${esc(countryName(c.country))}"><span class="flag">${flag(c.country)}</span>${esc(c.country === "?" ? "unknown" : c.country)}<b>${c.today}</b></div>`).join("")}</div>`
      : `<p class="muted" style="margin:0">Nobody has opened deskMonk yet today.</p>`}</div></div></div>`;
}

/** The land as one path of dots, built once per isolate: it never changes. */
let landPath = "";
function land(): string {
  if (landPath) return landPath;
  const parts: string[] = [];
  LAND_ROWS.forEach((hex, y) => {
    const bits = [...hex].map((c) => parseInt(c, 16).toString(2).padStart(4, "0")).join("");
    for (let x = 0; x < bits.length; x++) if (bits[x] === "1") parts.push(`M${x + 0.5} ${y + 0.5}h0`);
  });
  landPath = parts.join("");
  return landPath;
}

/** The dotted world with a glowing point on each country seen today, sized by how many.
 *  Countries without a known point ("?", Tor) are only in the list beside it. */
export function worldMap(list: Row[]): string {
  const width = LAND_ROWS[0].length * 4;
  const peak = Math.max(1, ...list.map((c) => c.today));
  const spots = list
    .map((c) => {
      const at = COUNTRY_POINTS[c.country];
      if (!at) return "";
      const x = ((at[0] + 180) / MAP_STEP).toFixed(1);
      const y = ((MAP_NORTH - at[1]) / MAP_STEP + 0.5).toFixed(1);
      const size = 0.8 + Math.sqrt(c.today / peak) * 1.1;
      return `<g class="spot"><title>${esc(countryName(c.country))}: ${c.today}</title>
        <circle class="halo" cx="${x}" cy="${y}" r="${(size * 2.6).toFixed(2)}"/><circle class="core" cx="${x}" cy="${y}" r="${size.toFixed(2)}"/></g>`;
    })
    .join("");
  return `<svg class="world" viewBox="0 0 ${width} ${LAND_ROWS.length}" role="img" aria-label="Map of today's active users">
    <path class="land" d="${land()}"/>${spots}</svg>`;
}

// ---------- page frame ----------

const ICON_PATHS: Record<string, string> = {
  grid: '<rect width="7" height="7" x="3" y="3" rx="1"/><rect width="7" height="7" x="14" y="3" rx="1"/><rect width="7" height="7" x="14" y="14" rx="1"/><rect width="7" height="7" x="3" y="14" rx="1"/>',
  chart: '<path d="M3 3v18h18"/><path d="M18 17V9"/><path d="M13 17V5"/><path d="M8 17v-3"/>',
  bars: '<path d="M6 20v-6"/><path d="M12 20V8"/><path d="M18 20V4"/>',
  userPlus: '<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M19 8v6"/><path d="M22 11h-6"/>',
  users: '<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>',
  cart: '<circle cx="8" cy="21" r="1"/><circle cx="19" cy="21" r="1"/><path d="M2.05 2.05h2l2.66 12.42a2 2 0 0 0 2 1.58h9.78a2 2 0 0 0 1.95-1.57l1.65-7.43H5.12"/>',
  globe: '<circle cx="12" cy="12" r="10"/><path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20"/><path d="M2 12h20"/>',
  download: '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m7 10 5 5 5-5"/><path d="M12 15V3"/>',
  clock: '<circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/>',
  laptop: '<rect width="16" height="11" x="4" y="4" rx="2"/><path d="M2 20h20"/>',
  file: '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M16 13H8"/><path d="M16 17H8"/>',
  card: '<rect width="20" height="14" x="2" y="5" rx="2"/><path d="M2 10h20"/>',
  key: '<circle cx="7.5" cy="15.5" r="5.5"/><path d="m21 2-9.6 9.6"/><path d="m15.5 7.5 3 3L22 7l-3-3"/>',
  wallet: '<path d="M12 2v20"/><path d="M17 5H9.5a3.5 3.5 0 0 0 0 7h5a3.5 3.5 0 0 1 0 7H6"/>',
  tools: '<path d="M4 21v-7"/><path d="M4 10V3"/><path d="M12 21v-9"/><path d="M12 8V3"/><path d="M20 21v-5"/><path d="M20 12V3"/><path d="M2 14h4"/><path d="M10 8h4"/><path d="M18 16h4"/>',
  calendar: '<rect width="18" height="18" x="3" y="4" rx="2"/><path d="M16 2v4"/><path d="M8 2v4"/><path d="M3 10h18"/>',
  star: '<path d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01z"/>',
  broom: '<path d="m13 11 9-9"/><path d="M14.6 12.6c.8.8.9 2.1.2 3L10 22l-8-8 6.4-4.8c.9-.7 2.2-.6 3 .2Z"/><path d="m6.8 10.4 6.8 6.8"/><path d="m5 17 1.5 1.5"/>',
  apple: '<path d="M12 6.5V3a1 1 0 0 1 1-1"/><path d="M18.2 21A15 15 0 0 0 22 11a6 6 0 0 0-10-4.5A6 6 0 0 0 2 11a15 15 0 0 0 3.8 10 3 3 0 0 0 3.6.6 5.5 5.5 0 0 1 5.2 0 3 3 0 0 0 3.6-.6"/>',
};

const icon = (name: string) => `<svg class="i" viewBox="0 0 24 24" aria-hidden="true">${ICON_PATHS[name] ?? ""}</svg>`;
const WINDOWS_ICON = `<svg viewBox="0 0 24 24" aria-hidden="true" fill="currentColor" class="win"><path d="M3 5.5 10.5 4.4v7.1H3zM11.5 4.3 21 3v8.5h-9.5zM3 12.5h7.5v7.1L3 18.5zM11.5 12.5H21V21l-9.5-1.3z"/></svg>`;

/** Lights the section on screen in the side list, opens folded sections when linked, `/` jumps to the search. */
const SHELL_SCRIPT = `(() => {
  const links = [...document.querySelectorAll('.side a[href^="#"]')];
  const open = (id) => { const el = document.getElementById(id); if (el && el.tagName === 'DETAILS') el.open = true; };
  for (const a of links) a.addEventListener('click', () => open(a.hash.slice(1)));
  if (location.hash) open(location.hash.slice(1));
  const zones = links.map((a) => document.getElementById(a.hash.slice(1))).filter(Boolean);
  const light = () => {
    let current = zones[0];
    for (const z of zones) if (z.getBoundingClientRect().top < 140) current = z;
    if (innerHeight + scrollY >= document.body.scrollHeight - 4) current = zones[zones.length - 1];
    for (const a of links) a.classList.toggle('on', current && a.hash === '#' + current.id);
  };
  addEventListener('scroll', light, { passive: true });
  light();
  addEventListener('keydown', (e) => {
    if (e.key !== '/' || /INPUT|TEXTAREA|SELECT/.test(document.activeElement?.tagName ?? '')) return;
    e.preventDefault();
    const q = document.getElementById('q');
    q?.scrollIntoView({ block: 'center' });
    q?.focus();
  });
})();`;

function layout(title: string, body: string): string {
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="robots" content="noindex"><title>${title}</title><link rel="icon" href="/logo.svg"><style>${STYLE}</style></head><body>${body}</body></html>`;
}

const STYLE = `
:root { color-scheme: dark; --bg:#0f1117; --card:#171a23; --card-2:#1d212c; --border:#262b38; --text:#e6e8ef; --muted:#8a91a5; --faint:#5b6275;
  --brand:#7c8cff; --accent:#4ade80; --red:#f87171; --orange:#fb923c; }
* { box-sizing: border-box; }
body { margin: 0; font: 14px/1.5 Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif; color: var(--text); background: var(--bg); }
a { color: var(--brand); }
code { color: var(--brand); background: rgba(124,140,255,.1); padding: 1px 5px; border-radius: 5px; font-size: 12.5px; }
code.k { letter-spacing: .5px; user-select: all; }
.appbar { position: sticky; top: 0; z-index: 5; display: flex; align-items: center; gap: 12px; padding: 8px 24px;
  background: rgba(15,17,23,.92); backdrop-filter: blur(8px); border-bottom: 1px solid var(--border); }
.appbar .brand { display: flex; align-items: center; gap: 10px; font-size: 20px; font-weight: 700; color: var(--text); text-decoration: none; }
.appbar .brand img { width: 32px; height: 32px; border-radius: 8px; }
.appbar .tag { font-size: 11px; font-weight: 600; letter-spacing: .6px; color: var(--muted); background: var(--card-2); border-radius: 6px; padding: 4px 8px; }
.appbar .spacer { flex: 1; }
.appbar a.btn, .appbar .env { display: inline-flex; align-items: center; gap: 7px; font-size: 13px; color: var(--text); text-decoration: none;
  border: 1px solid var(--border); border-radius: 9px; padding: 5px 11px; }
.appbar a.btn:hover { border-color: var(--brand); }
.appbar .env::before { content: ""; width: 8px; height: 8px; border-radius: 50%; background: var(--orange); }
.appbar .env.prod { color: var(--accent); border-color: rgba(74,222,128,.45); }
.appbar .env.prod::before { background: var(--accent); }
.shell { display: grid; grid-template-columns: 200px minmax(0, 1fr); gap: 22px; padding: 0 24px; }
.side { position: sticky; top: 56px; align-self: start; padding: 12px 0; display: flex; flex-direction: column; gap: 4px; }
.side a { display: flex; align-items: center; gap: 10px; color: var(--muted); text-decoration: none; font-size: 14px; padding: 8px 12px;
  border-radius: 10px; border: 1px solid transparent; }
.side a:hover { color: var(--text); background: var(--card); }
.side a.on { color: var(--brand); background: rgba(124,140,255,.1); border-color: rgba(124,140,255,.25); }
.side a .count { margin-left: auto; font-size: 11px; color: var(--muted); }
.side a .count.warn { color: #0f1117; background: var(--orange); border-radius: 999px; padding: 0 7px; }
.side .hint { font-size: 11px; color: var(--faint); padding: 14px 12px 0; }
kbd { font-family: inherit; border: 1px solid var(--border); border-radius: 4px; padding: 0 5px; }
.main { padding: 12px 0 40px; min-width: 0; }
.main.solo { max-width: 1200px; margin: 0 auto; padding: 12px 24px 40px; }
h1.title { font-size: 22px; margin: 4px 0; }
.zone, .card { scroll-margin-top: 76px; }
.card { background: var(--card); border: 1px solid var(--border); border-radius: 14px; padding: 16px 20px; margin-bottom: 16px; }
.label { font-size: 12px; font-weight: 600; color: var(--muted); text-transform: uppercase; letter-spacing: .6px; margin: 0 0 10px; }
.label .aside { text-transform: none; font-weight: 400; letter-spacing: 0; color: var(--faint); }
.grid { display: grid; gap: 12px; grid-template-columns: repeat(auto-fit, minmax(210px, 1fr)); }
.grid.five { grid-template-columns: repeat(auto-fit, minmax(175px, 1fr)); }
.stat { display: flex; flex-direction: column; background: var(--card-2); border: 1px solid var(--border); border-radius: 12px; padding: 11px 14px; }
.stat .head { display: flex; gap: 12px; align-items: flex-start; }
.stat .k { margin: 0; font-size: 14px; font-weight: 600; line-height: 1.3; }
.stat.plain .k { color: var(--muted); font-weight: 500; font-size: 13px; }
.stat .k small { display: block; font-size: 12px; font-weight: 400; color: var(--muted); margin-top: 1px; }
.stat .k .info { color: var(--faint); cursor: help; font-weight: 400; }
.stat .v { font-size: 30px; font-weight: 800; line-height: 1.15; margin-top: 6px; font-variant-numeric: tabular-nums; }
.stat .d { font-size: 12.5px; color: var(--muted); margin: 4px 0 0; }
.stat.mine { border-color: rgba(124,140,255,.55); }
.stat.alert { box-shadow: inset 3px 0 0 var(--orange); }
.stat.alert .v { color: var(--orange); }
.up { color: var(--accent); } .down { color: var(--red); } .muted { color: var(--muted); }
.ico { display: inline-flex; align-items: center; justify-content: center; width: 34px; height: 34px; border-radius: 10px; flex: none; }
svg.i { width: 18px; height: 18px; flex: none; fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; vertical-align: -4px; }
svg.win { width: 13px; height: 13px; vertical-align: -2px; color: #3b9cff; }
.t-blue { color: #60a5fa; background: rgba(96,165,250,.14); } .t-red { color: #f87171; background: rgba(248,113,113,.14); }
.t-green { color: #4ade80; background: rgba(74,222,128,.13); } .t-teal { color: #2dd4bf; background: rgba(45,212,191,.13); }
.t-violet { color: #a78bfa; background: rgba(167,139,250,.15); } .t-orange { color: #fb923c; background: rgba(251,146,60,.14); }
.osbars { border-top: 1px solid var(--border); margin-top: 8px; padding-top: 7px; display: grid; gap: 3px; }
.osrow { display: grid; grid-template-columns: 18px 70px minmax(26px, auto) 1fr; align-items: center; gap: 8px; font-size: 12.5px; line-height: 1.35; }
.osrow .nm { color: var(--muted); } .osrow b { font-variant-numeric: tabular-nums; } .osrow.zero b { color: var(--faint); font-weight: 500; }
.osrow .ic { display: inline-flex; justify-content: center; font-size: 14px; line-height: 1; }
.osrow .ic svg { width: 15px; height: 15px; }
.track { display: block; height: 6px; background: var(--bg); border-radius: 99px; overflow: hidden; }
.track i { display: block; height: 100%; border-radius: 99px; background: var(--brand); }
.os-windows .track i { background: #3b9cff; } .os-linux .track i { background: #f59e0b; } .os-macos .track i { background: #a855f7; }
.os-macos .ic { color: #e6e8ef; } .os-other .track i { background: var(--muted); }
.c-web .track i, .c-web .ic { background: #60a5fa; color: #60a5fa; } .c-web .track i { background: #60a5fa; }
.c-store .ic { color: #a855f7; } .c-store .track i { background: #a855f7; }
.c-manual .ic { color: var(--orange); } .c-manual .track i { background: var(--orange); }
.c-web .ic { background: none; }
.stat .foot-line { display: flex; gap: 8px; align-items: flex-start; margin: auto 0 0; padding-top: 7px; border-top: 1px solid var(--border);
  margin-top: 8px; font-size: 12px; line-height: 1.4; color: var(--muted); }
.stat .foot-line svg.i { width: 15px; height: 15px; margin-top: 1px; }
.bars { display: flex; align-items: flex-end; gap: 4px; height: 80px; margin-top: 12px; }
.bars div { flex: 1; background: var(--brand); opacity: .7; border-radius: 3px 3px 0 0; min-height: 2px; }
.bars div:last-child { opacity: 1; }
.bars-axis { display: flex; justify-content: space-between; font-size: 11px; color: var(--faint); margin-top: 4px; }
.hbars { display: grid; gap: 8px; }
.hbar { display: grid; grid-template-columns: 130px 1fr 90px; gap: 12px; align-items: center; }
.hbar .track { height: 10px; } .hbar b { text-align: right; font-variant-numeric: tabular-nums; }
.places { margin-top: 12px; background: var(--card-2); border: 1px solid var(--border); border-radius: 12px; padding: 10px 14px; }
.places .title { display: flex; align-items: center; gap: 10px; font-size: 14px; font-weight: 600; margin: 0 0 8px; }
.places .title svg { color: var(--brand); } .places .title span { font-weight: 400; color: var(--muted); }
.places .list { display: grid; gap: 8px; grid-template-columns: repeat(auto-fill, minmax(110px, 1fr)); }
.place { display: flex; align-items: center; gap: 8px; background: var(--bg); border: 1px solid var(--border); border-radius: 9px; padding: 5px 10px; font-size: 13px; }
.place .flag { font-size: 17px; line-height: 1; } .place b { margin-left: auto; font-variant-numeric: tabular-nums; }
.places .split { display: grid; gap: 18px; align-items: center; grid-template-columns: auto minmax(0, 1fr); }
svg.world { height: 128px; width: auto; max-width: 100%; display: block; }
svg.world .land { fill: none; stroke: #2f3550; stroke-width: .62; stroke-linecap: round; }
svg.world .halo { fill: var(--brand); opacity: .22; animation: halo 2.6s ease-in-out infinite; transform-box: fill-box; transform-origin: center; }
svg.world .core { fill: #c3caff; filter: drop-shadow(0 0 1.2px var(--brand)); }
svg.world .spot:hover .core { fill: #fff; }
@keyframes halo { 0%, 100% { transform: scale(.8); opacity: .3; } 50% { transform: scale(1.15); opacity: .12; } }
@media (prefers-reduced-motion: reduce) { svg.world .halo { animation: none; } }
.scroll { overflow-x: auto; }
table { border-collapse: collapse; width: 100%; font-size: 13px; }
th, td { border-bottom: 1px solid var(--border); padding: 8px 10px; text-align: left; white-space: nowrap; }
th { color: var(--muted); font-size: 11.5px; font-weight: 600; }
td.num, th.num { text-align: right; font-variant-numeric: tabular-nums; }
tr:last-child td { border-bottom: 0; }
table tr:hover td { background: rgba(124,140,255,.05); }
tr.total td { border-top: 1px solid var(--border); font-weight: 600; }
tr.mine td { opacity: .6; }
.foot { font-size: 12.5px; margin: 10px 0 0; }
details.card { padding: 0; }
details.card > summary { cursor: pointer; padding: 15px 20px; font-weight: 600; list-style: none; }
details.card > summary::-webkit-details-marker { display: none; }
details.card > summary::before { content: "▸ "; color: var(--muted); }
details.card[open] > summary::before { content: "▾ "; }
details.card > .body { padding: 0 20px 18px; }
details.card.warnbox > summary { color: var(--orange); }
details.act summary { cursor: pointer; color: var(--brand); list-style: none; font-size: 12.5px; }
details.act summary::-webkit-details-marker { display: none; }
.acts { display: flex; flex-wrap: wrap; gap: 6px; padding-top: 8px; }
.badge { display: inline-block; font-size: 11px; border-radius: 999px; padding: 2px 8px; background: var(--card-2); color: var(--muted); white-space: nowrap; }
.badge.on { background: rgba(74,222,128,.15); color: var(--accent); } .badge.off { color: var(--faint); }
.badge.new { background: rgba(124,140,255,.18); color: var(--brand); } .badge.warn { background: rgba(251,146,60,.15); color: var(--orange); }
.badge.you, .badge.manual { background: rgba(251,146,60,.15); color: var(--orange); }
.badge.store { background: rgba(168,85,247,.16); color: #c084fc; }
.group { border-left: 3px solid var(--border); padding-left: 14px; margin-top: 20px; }
.group.paddle { border-left-color: var(--brand); } .group.manual { border-left-color: var(--orange); }
.group-head { margin: 0 0 8px; display: flex; gap: 8px; align-items: center; }
.row { display: flex; flex-wrap: wrap; gap: 12px; align-items: flex-end; }
label.field { display: block; color: var(--muted); font-size: 12px; margin-bottom: 4px; }
input, select { font: inherit; background: var(--bg); color: var(--text); border: 1px solid var(--border); border-radius: 8px; padding: 9px 12px; width: 100%; }
input:focus, select:focus { outline: none; border-color: var(--brand); }
form.inline { display: inline-flex; gap: 6px; align-items: center; margin: 0; }
form.inline input { width: 72px; padding: 5px 8px; font-size: 12.5px; }
button { font: inherit; font-weight: 600; font-size: 13px; color: #0f1117; background: var(--brand); border: 0; border-radius: 8px; padding: 9px 14px; cursor: pointer; white-space: nowrap; }
button:hover { filter: brightness(1.1); }
button.quiet { background: transparent; border: 1px solid var(--border); color: var(--text); }
button.danger { background: var(--red); }
form.inline button { font-size: 12px; padding: 5px 10px; }
.btn-link { align-self: center; }
.note { border-radius: 10px; padding: 10px 14px; margin: 12px 0; }
.note.ok { background: rgba(74,222,128,.1); color: var(--accent); } .note.err { background: rgba(248,113,113,.12); color: var(--red); }
.term { background: #070b08; color: #39ff14; border: 1px solid #1e5c2b; border-radius: 12px; padding: 16px 18px; overflow-x: auto; margin: 0 0 16px;
  font: 13px/1.6 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; text-shadow: 0 0 6px rgba(57,255,20,.35); }
.term .dim { color: #1f9e45; } .term .k { color: #1f9e45; display: inline-block; min-width: 12ch; } .term .b { font-weight: 700; }
.term .cur { animation: blink 1s steps(1) infinite; } @keyframes blink { 50% { opacity: 0; } }
@media (min-width: 901px) and (max-width: 1299px) {
  .shell { grid-template-columns: 52px minmax(0, 1fr); gap: 16px; }
  .side a { justify-content: center; padding: 10px 0; position: relative; }
  .side a .nav-label, .side .hint { display: none; }
  .side a .count { position: absolute; top: 2px; right: 2px; margin: 0; font-size: 10px; }
}
@media (max-width: 900px) {
  .shell { grid-template-columns: minmax(0, 1fr); padding: 0 16px; }
  .side { flex-direction: row; overflow-x: auto; top: 50px; z-index: 4; background: var(--bg); padding: 8px 0; }
  .side a { white-space: nowrap; padding: 7px 10px; } .side .hint { display: none; }
  .appbar { padding: 8px 16px; } .appbar a.btn span { display: none; }
  .main.solo { padding: 12px 16px 40px; }
  .hbar { grid-template-columns: 100px 1fr 70px; }
  .places .split { grid-template-columns: minmax(0, 1fr); }
  svg.world { width: 100%; height: auto; }
}
`;
