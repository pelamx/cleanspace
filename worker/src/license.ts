//! deskMonk Pro: Paddle checkout, Paddle webhooks and license checks for the apps.
//! GET  /buy?install=<id>&plan=monthly|yearly   checkout page (Paddle overlay), the app opens it in the browser
//! GET  /buy/done?txn=<transaction id>          after paying: shows the license key
//! POST /api/paddle                             Paddle webhook (subscription.*), signature checked
//! POST /api/license/validate {key, install_id} → {key, pro_until}; a key works on up to MAX_DEVICES installations
//! POST /api/license/portal {key}               → {url} of Paddle's page to cancel or change the subscription

export interface LicenseEnv {
  DB: D1Database;
  PADDLE_ENV?: string;
  PADDLE_CLIENT_TOKEN?: string;
  PADDLE_API_KEY?: string;
  PADDLE_WEBHOOK_SECRET?: string;
  PADDLE_PRICE_MONTHLY?: string;
  PADDLE_PRICE_YEARLY?: string;
}

export const MAX_DEVICES = 3;
/** Webhooks older than this are refused (replays). */
const SIGNATURE_TOLERANCE = 300;
const INSTALL_ID = /^[A-Za-z0-9._-]{1,64}$/;
const KEY = /^DM(-[A-HJ-NP-Z2-9]{4}){3}$/;
const KEY_CHARS = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/** Returns a response for license routes, or null for any other path. */
export async function handleLicense(req: Request, env: LicenseEnv, path: string): Promise<Response | null> {
  if (req.method === "GET" && path === "/buy") return buyPage(env, new URL(req.url));
  if (req.method === "GET" && path === "/buy/done") return donePage(env, new URL(req.url).searchParams.get("txn") ?? "");
  if (req.method === "POST" && path === "/api/paddle") return webhook(req, env);
  if (req.method === "POST" && (path === "/api/license/validate" || path === "/api/license/portal")) {
    const body = await req.text();
    if (body.length > 1024) return json({ error: "bad_request" }, 400);
    let v: any;
    try {
      v = JSON.parse(body);
    } catch {
      return json({ error: "bad_request" }, 400);
    }
    const key = typeof v?.key === "string" ? v.key.trim().toUpperCase() : "";
    if (key !== "" && !KEY.test(key)) return json({ error: "unknown_key" }, 404);
    if (path === "/api/license/portal") return portal(env, key);
    const install = typeof v?.install_id === "string" && INSTALL_ID.test(v.install_id) ? v.install_id : null;
    if (install === null) return json({ error: "bad_request" }, 400);
    return validate(env.DB, key, install);
  }
  return null;
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

// ---------- pure parts (tested) ----------

/** Checks a `Paddle-Signature: ts=…;h1=…` header: HMAC-SHA256 of "ts:body" with the webhook secret. */
export async function verifySignature(header: string, body: string, secret: string, now: number): Promise<boolean> {
  const parts = header.split(";").map((p) => p.split("="));
  const ts = parts.find(([k]) => k === "ts")?.[1] ?? "";
  const sigs = parts.filter(([k]) => k === "h1").map(([, v]) => v);
  if (!/^\d+$/.test(ts) || Math.abs(now - Number(ts)) > SIGNATURE_TOLERANCE || sigs.length === 0) return false;
  const enc = new TextEncoder();
  const hmacKey = await crypto.subtle.importKey("raw", enc.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", hmacKey, enc.encode(`${ts}:${body}`)));
  const hex = [...mac].map((b) => b.toString(16).padStart(2, "0")).join("");
  // Constant time per candidate; there are at most two during a secret rotation.
  return sigs.some((s) => s.length === hex.length && [...s].reduce((acc, c, i) => acc | (c.charCodeAt(0) ^ hex.charCodeAt(i)), 0) === 0);
}

/** End of the paid period (unix seconds) of a Paddle subscription; 0 = not Pro. */
export function periodEnd(sub: any): number {
  if (!["active", "trialing", "past_due"].includes(sub?.status)) return 0;
  const end = Date.parse(sub?.current_billing_period?.ends_at ?? "");
  return Number.isFinite(end) ? Math.floor(end / 1000) : 0;
}

export function newKey(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(12));
  const chars = [...bytes].map((b) => KEY_CHARS[b % KEY_CHARS.length]).join("");
  return `DM-${chars.slice(0, 4)}-${chars.slice(4, 8)}-${chars.slice(8)}`;
}

/** Whether `install` may use a key already active on `devices`. */
export function mayActivate(devices: string[], install: string): boolean {
  return devices.includes(install) || devices.length < MAX_DEVICES;
}

// ---------- routes ----------

async function webhook(req: Request, env: LicenseEnv): Promise<Response> {
  const body = await req.text();
  const secret = env.PADDLE_WEBHOOK_SECRET ?? "";
  if (secret === "" || !(await verifySignature(req.headers.get("Paddle-Signature") ?? "", body, secret, Math.floor(Date.now() / 1000)))) {
    return new Response("bad signature", { status: 401 });
  }
  const ev = JSON.parse(body);
  if (typeof ev?.event_type !== "string" || !ev.event_type.startsWith("subscription.")) return new Response(null, { status: 200 });
  const sub = ev.data;
  if (typeof sub?.id !== "string" || typeof sub?.customer_id !== "string") return new Response("bad event", { status: 400 });
  const install = typeof sub.custom_data?.install_id === "string" && INSTALL_ID.test(sub.custom_data.install_id) ? sub.custom_data.install_id : null;
  const txn = typeof sub.transaction_id === "string" ? sub.transaction_id : null;
  const key = newKey();
  // Events can arrive out of order: an older one never overwrites a newer state. The key is made once.
  await env.DB.prepare(
    `INSERT INTO subscriptions(id, customer_id, status, period_end, license_key, install_id, txn, event_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
     ON CONFLICT(id) DO UPDATE SET customer_id = excluded.customer_id, status = excluded.status, period_end = excluded.period_end,
       install_id = COALESCE(subscriptions.install_id, excluded.install_id), txn = COALESCE(subscriptions.txn, excluded.txn), event_at = excluded.event_at
     WHERE excluded.event_at >= subscriptions.event_at`,
  )
    .bind(sub.id, sub.customer_id, String(sub.status), periodEnd(sub), key, install, txn, String(ev.occurred_at ?? ""))
    .run();
  if (install !== null) {
    await env.DB.prepare(
      `INSERT OR IGNORE INTO activations(license_key, install_id, ts) SELECT license_key, ?2, ?3 FROM subscriptions WHERE id = ?1`,
    )
      .bind(sub.id, install, Math.floor(Date.now() / 1000))
      .run();
  }
  return new Response(null, { status: 200 });
}

async function validate(db: D1Database, key: string, install: string): Promise<Response> {
  const sub =
    key !== ""
      ? await db.prepare("SELECT license_key, period_end FROM subscriptions WHERE license_key = ?1").bind(key).first<{ license_key: string; period_end: number }>()
      : // No key yet: a purchase made from this installation (the app checks right after the checkout).
        await db
          .prepare("SELECT license_key, period_end FROM subscriptions WHERE install_id = ?1 ORDER BY period_end DESC LIMIT 1")
          .bind(install)
          .first<{ license_key: string; period_end: number }>();
  if (sub === null) return key !== "" ? json({ error: "unknown_key" }, 404) : json({ key: "", pro_until: 0 });
  const devices = (await db.prepare("SELECT install_id FROM activations WHERE license_key = ?1").bind(sub.license_key).raw<[string]>()).map(([id]) => id);
  if (!mayActivate(devices, install)) return json({ error: "too_many_devices" }, 403);
  if (!devices.includes(install)) {
    await db
      .prepare("INSERT OR IGNORE INTO activations(license_key, install_id, ts) VALUES (?1, ?2, ?3)")
      .bind(sub.license_key, install, Math.floor(Date.now() / 1000))
      .run();
  }
  return json({ key: sub.license_key, pro_until: sub.period_end });
}

function paddleApi(env: LicenseEnv): string {
  return env.PADDLE_ENV === "production" ? "https://api.paddle.com" : "https://sandbox-api.paddle.com";
}

async function portal(env: LicenseEnv, key: string): Promise<Response> {
  if (key === "") return json({ error: "unknown_key" }, 404);
  const sub = await env.DB.prepare("SELECT id, customer_id FROM subscriptions WHERE license_key = ?1").bind(key).first<{ id: string; customer_id: string }>();
  if (sub === null) return json({ error: "unknown_key" }, 404);
  const res = await fetch(`${paddleApi(env)}/customers/${encodeURIComponent(sub.customer_id)}/portal-sessions`, {
    method: "POST",
    headers: { Authorization: `Bearer ${env.PADDLE_API_KEY ?? ""}`, "Content-Type": "application/json" },
    body: JSON.stringify({ subscription_ids: [sub.id] }),
  });
  const url = res.ok ? ((await res.json()) as any)?.data?.urls?.general?.overview : undefined;
  return typeof url === "string" ? json({ url }) : json({ error: "paddle" }, 502);
}

function page(title: string, body: string, head = ""): Response {
  const html = `<!doctype html><html><head><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1"><title>${title}</title>${head}
<style>:root{color-scheme:light dark;--bg:#f7f7f8;--fg:#1d1d1f;--muted:#6b6b70;--accent:#4f6ef7;--card:#fff;--line:#e3e3e6}
@media(prefers-color-scheme:dark){:root{--bg:#151517;--fg:#ececee;--muted:#9a9aa0;--card:#1f1f22;--line:#333338}}
body{margin:0;min-height:100vh;display:grid;place-items:center;background:var(--bg);color:var(--fg);font:16px/1.5 system-ui,sans-serif;padding:16px;box-sizing:border-box}
main{max-width:440px;text-align:center}.muted{color:var(--muted)}h1{margin:.2em 0}
.key{font:600 22px ui-monospace,Menlo,monospace;letter-spacing:.06em;background:var(--card);border:1px solid var(--line);border-radius:12px;padding:14px;margin:18px 0;user-select:all}
button{font:inherit;font-weight:600;color:#fff;background:var(--accent);border:0;border-radius:10px;padding:10px 22px;cursor:pointer}</style></head>
<body><main><div style="font-size:44px">⭐</div>${body}</main></body></html>`;
  return new Response(html, { headers: { "Content-Type": "text/html; charset=utf-8" } });
}

function buyPage(env: LicenseEnv, url: URL): Response {
  const install = url.searchParams.get("install") ?? "";
  const price = url.searchParams.get("plan") === "yearly" ? env.PADDLE_PRICE_YEARLY : env.PADDLE_PRICE_MONTHLY;
  if (!INSTALL_ID.test(install)) return new Response("bad install id", { status: 400 });
  if (!price || !env.PADDLE_CLIENT_TOKEN) return new Response("checkout is not set up yet", { status: 503 });
  // Values go into a script as JSON; "<" is escaped so nothing can close the script tag.
  const cfg = JSON.stringify({ token: env.PADDLE_CLIENT_TOKEN, sandbox: env.PADDLE_ENV !== "production", price, install }).replace(/</g, "\\u003c");
  return page(
    "deskMonk Pro",
    `<h1>deskMonk Pro</h1><p class=muted>The secure Paddle checkout opens on this page.</p><button id=open>Open checkout</button>
<script src="https://cdn.paddle.com/paddle/v2/paddle.js"></script><script>
const c = ${cfg};
if (c.sandbox) Paddle.Environment.set("sandbox");
Paddle.Initialize({ token: c.token, eventCallback: (e) => {
  if (e.name === "checkout.completed") location.href = "/buy/done?txn=" + encodeURIComponent(e.data.transaction_id);
} });
const open = () => Paddle.Checkout.open({ items: [{ priceId: c.price, quantity: 1 }], customData: { install_id: c.install } });
document.getElementById("open").onclick = open;
open();
</script>`,
  );
}

async function donePage(env: LicenseEnv, txn: string): Promise<Response> {
  if (!/^txn_[a-z0-9]{10,40}$/.test(txn)) return new Response("bad transaction id", { status: 400 });
  const sub = await env.DB.prepare("SELECT license_key FROM subscriptions WHERE txn = ?1").bind(txn).first<{ license_key: string }>();
  // Paddle's webhook can land a few seconds after the checkout closes.
  if (sub === null) return page("deskMonk Pro", "<h1>Thank you!</h1><p class=muted>Finishing your purchase…</p>", '<meta http-equiv=refresh content=3>');
  return page(
    "deskMonk Pro",
    `<h1>Thank you!</h1><p>deskMonk turns Pro by itself in a few seconds. You can close this tab.</p>
<div class=key>${sub.license_key}</div>
<p class=muted>Your license key: use it under ⭐ Pro → "Have a license key?" on up to ${MAX_DEVICES} computers. Paddle also emails you a receipt.</p>`,
  );
}
