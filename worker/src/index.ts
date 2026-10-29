//! DeskMonk telemetry + admin panel on Cloudflare Workers and D1. Same routes as server/:
//! POST /api/events        ← apps send JSON events here
//! GET  /admin             overview (HTTP Basic auth: user "admin", password = ADMIN_TOKEN secret)
//! GET  /admin/user/<id>   one installation's full history and hardware

export interface Env {
  DB: D1Database;
  ADMIN_TOKEN: string;
}

// Room for an "open" event's hardware list.
const MAX_BODY = 16384;
const MAX_SPECS = 40;
const MAX_SPEC_TEXT = 200;
const SIZE_FIELDS = ["bytes", "duplicate_bytes", "junk_bytes", "old_download_bytes", "large_bytes"] as const;
const DAY = 86400;

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const path = new URL(req.url).pathname;

    if (req.method === "POST" && path === "/api/events") {
      const body = await req.text();
      if (body.length > MAX_BODY) return new Response("body too large", { status: 400 });
      let ev: Event;
      try {
        ev = parseEvent(body);
      } catch (e) {
        return new Response(String((e as Error).message), { status: 400 });
      }
      await insert(env.DB, ev);
      return new Response(null, { status: 204 });
    }

    if (req.method !== "GET" || !(path === "/admin" || path.startsWith("/admin/"))) {
      return new Response(null, { status: 404 });
    }
    const token = env.ADMIN_TOKEN ?? "";
    if (token.length < 12) return new Response("ADMIN_TOKEN must be set (at least 12 characters)", { status: 500 });
    if (!constantTimeEqual(req.headers.get("Authorization") ?? "", `Basic ${btoa(`admin:${token}`)}`)) {
      return new Response(null, { status: 401, headers: { "WWW-Authenticate": 'Basic realm="DeskMonk admin"' } });
    }
    const id = path.startsWith("/admin/user/") ? decodeURIComponent(path.slice("/admin/user/".length)) : null;
    const html = id === null ? await overviewPage(env.DB) : await userPage(env.DB, id);
    return new Response(html, { headers: { "Content-Type": "text/html; charset=utf-8" } });
  },
} satisfies ExportedHandler<Env>;

function constantTimeEqual(a: string, b: string): boolean {
  const [x, y] = [new TextEncoder().encode(a), new TextEncoder().encode(b)];
  return x.length === y.length && crypto.subtle.timingSafeEqual(x, y);
}

export interface Event {
  install_id: string;
  version: string;
  os: string;
  client: string;
  kind: string;
  files: number;
  sizes: number[];
  /** Latest hardware list (re-serialized validated JSON), sent with "open" events. */
  specs: string | null;
}

/** Validates untrusted input from the apps. Throws with the reason. */
export function parseEvent(body: string): Event {
  let v: any;
  try {
    v = JSON.parse(body);
  } catch (e) {
    throw new Error(`bad json: ${(e as Error).message}`);
  }
  if (typeof v !== "object" || v === null || Array.isArray(v)) throw new Error("bad json");
  const text = (key: string, max: number): string => {
    const s = v[key];
    if (typeof s !== "string") throw new Error(`missing ${key}`);
    if (!(s.length > 0 && s.length <= max && /^[A-Za-z0-9._-]+$/.test(s))) throw new Error(`invalid ${key}`);
    return s;
  };
  // Missing counts are 0 (clean events have no category breakdown); present ones must be non-negative integers.
  const count = (key: string): number => {
    const n = v[key];
    if (n === undefined || n === null) return 0;
    if (!Number.isSafeInteger(n) || n < 0) throw new Error(`invalid ${key}`);
    return n;
  };
  const kind = text("kind", 16);
  if (!["scan", "clean", "open"].includes(kind)) throw new Error("invalid kind");
  const sizes = SIZE_FIELDS.map(count);
  // Free text from the OS (model names); shown escaped, so only length and control characters are limited.
  let specs: string | null = null;
  if (v.specs !== undefined && v.specs !== null) {
    if (!(Array.isArray(v.specs) && kind === "open" && v.specs.length <= MAX_SPECS)) throw new Error("invalid specs");
    specs = JSON.stringify(
      v.specs.map((spec: any) => {
        const obj: Record<string, string> = {};
        for (const key of ["kind", "brand", "name", "detail"]) {
          const s = spec?.[key];
          if (typeof s !== "string" || [...s].length > MAX_SPEC_TEXT || /\p{Cc}/u.test(s)) throw new Error("invalid specs");
          obj[key] = s;
        }
        return obj;
      }),
    );
  }
  return {
    install_id: text("install_id", 64),
    version: text("version", 32),
    os: text("os", 32),
    client: text("client", 16),
    kind,
    files: count("files"),
    sizes,
    specs,
  };
}

async function insert(db: D1Database, e: Event): Promise<void> {
  const now = Math.floor(Date.now() / 1000);
  const stmts = [
    db
      .prepare(
        `INSERT INTO events(ts, install_id, version, os, client, kind, files, bytes, duplicate_bytes, junk_bytes, old_download_bytes, large_bytes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)`,
      )
      .bind(now, e.install_id, e.version, e.os, e.client, e.kind, e.files, ...e.sizes),
  ];
  if (e.specs !== null) {
    stmts.push(
      db
        .prepare(
          `INSERT INTO hardware(install_id, ts, specs) VALUES (?1, ?2, ?3)
           ON CONFLICT(install_id) DO UPDATE SET ts = excluded.ts, specs = excluded.specs`,
        )
        .bind(e.install_id, now, e.specs),
    );
  }
  await db.batch(stmts);
}

// ---------- admin HTML ----------

export function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&#39;");
}

export function humanSize(bytes: number): string {
  const UNITS = ["B", "KB", "MB", "GB", "TB"];
  let [size, unit] = [bytes, 0];
  while (size >= 1024 && unit < UNITS.length - 1) {
    size /= 1024;
    unit++;
  }
  return unit === 0 ? `${bytes} B` : `${size.toFixed(1)} ${UNITS[unit]}`;
}

/** Renders a query as an HTML table. Columns named in `heads` ending with " (boyut)" are shown as sizes;
 *  with `link`, the first column is an install id linking to its detail page. */
async function table(db: D1Database, title: string, sql: string, args: unknown[], heads: string[], link: boolean): Promise<string> {
  let out = `<h2>${esc(title)}</h2><div class=scroll><table><tr>`;
  for (const h of heads) out += `<th>${esc(h.replace(/ \(boyut\)$/, ""))}</th>`;
  out += "</tr>";
  const rows = await db.prepare(sql).bind(...args).raw();
  for (const row of rows) {
    out += "<tr>";
    row.forEach((value, i) => {
      const num = typeof value === "number";
      const cell =
        value === null ? "" : num && heads[i].endsWith("(boyut)") ? humanSize(value) : num ? (Number.isInteger(value) ? String(value) : value.toFixed(1)) : esc(String(value));
      out += link && i === 0 ? `<td><a href="/admin/user/${cell}">${cell}</a></td>` : num ? `<td class=n>${cell}</td>` : `<td>${cell}</td>`;
    });
    out += "</tr>";
  }
  if (rows.length === 0) out += `<tr><td colspan=${heads.length}>Henüz veri yok</td></tr>`;
  return out + "</table></div>";
}

function layout(title: string, body: string): string {
  return `<!doctype html><html lang=tr><head><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1">
<title>${title}</title><style>
:root{color-scheme:light dark;--bg:#f7f7f8;--fg:#1d1d1f;--card:#fff;--line:#e3e3e6;--muted:#6b6b70;--accent:#2563eb}
@media(prefers-color-scheme:dark){:root{--bg:#151517;--fg:#ececee;--card:#1f1f22;--line:#333338;--muted:#9a9aa0;--accent:#6ea0ff}}
body{margin:0;padding:24px 16px;background:var(--bg);color:var(--fg);font:14px/1.45 system-ui,sans-serif}
main{max-width:1200px;margin:auto}a{color:var(--accent)}h1{margin:0 0 4px}h2{margin:28px 0 8px;font-size:16px}
.muted{color:var(--muted)}.cards{display:grid;grid-template-columns:repeat(auto-fill,minmax(170px,1fr));gap:12px;margin-top:16px}
.card{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:12px 14px}
.card b{display:block;font-size:22px;font-variant-numeric:tabular-nums}.card span{color:var(--muted);font-size:12px}
.scroll{overflow-x:auto;background:var(--card);border:1px solid var(--line);border-radius:10px}
table{border-collapse:collapse;width:100%}th,td{padding:7px 10px;border-bottom:1px solid var(--line);text-align:left;white-space:nowrap}
th{font-size:12px;color:var(--muted);font-weight:600}td.n{text-align:right;font-variant-numeric:tabular-nums}tr:last-child td{border-bottom:0}
.term{background:#070b08;color:#39ff14;border:1px solid #1e5c2b;border-radius:10px;padding:16px 18px;overflow-x:auto;
font:13px/1.6 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;text-shadow:0 0 6px rgba(57,255,20,.35)}
.term .dim{color:#1f9e45}.term .k{color:#1f9e45;display:inline-block;min-width:12ch}.term .b{font-weight:700}
.term .cur{animation:blink 1s steps(1) infinite}@keyframes blink{50%{opacity:0}}
</style></head><body><main>${body}</main></body></html>`;
}

async function cards(db: D1Database, sql: string, args: unknown[], labels: string[]): Promise<string> {
  const [row] = await db.prepare(sql).bind(...args).raw<(number | null)[]>();
  let out = "<div class=cards>";
  labels.forEach((label, i) => {
    const n = row?.[i] ?? 0;
    const value = label.endsWith("(boyut)") ? humanSize(n) : String(n);
    out += `<div class=card><b>${value}</b><span>${esc(label.replace(/ \(boyut\)$/, ""))}</span></div>`;
  });
  return out + "</div>";
}

async function overviewPage(db: D1Database): Promise<string> {
  const now = Math.floor(Date.now() / 1000);
  const [week, month] = [now - 7 * DAY, now - 30 * DAY];
  let body = "<h1>deskMonk by pelamx — Admin</h1><div class=muted>Anonim kullanım istatistikleri</div>";
  body += await cards(
    db,
    `SELECT COUNT(DISTINCT install_id),
            COUNT(DISTINCT CASE WHEN ts >= ?1 THEN install_id END),
            COUNT(DISTINCT CASE WHEN ts >= ?2 THEN install_id END),
            SUM(kind='scan'), SUM(kind='clean'),
            SUM(CASE WHEN kind='scan' THEN bytes END), SUM(CASE WHEN kind='clean' THEN bytes END),
            SUM(CASE WHEN kind='clean' THEN files END), SUM(kind='open')
     FROM events`,
    [week, month],
    ["Toplam kullanıcı", "Aktif (7 gün)", "Aktif (30 gün)", "Tarama", "Temizleme", "Bulunan alan (boyut)", "Kazanılan alan (boyut)", "Silinen dosya", "Uygulama açılışı"],
  );
  const groupBy = (col: string) =>
    `SELECT ${col}, COUNT(DISTINCT install_id), SUM(kind='scan'), SUM(kind='clean'),
            SUM(CASE WHEN kind='clean' THEN bytes ELSE 0 END)
     FROM events GROUP BY ${col} ORDER BY 2 DESC`;
  const heads = ["Kullanıcı", "Tarama", "Temizleme", "Kazanılan (boyut)"];
  body += await table(db, "İşletim sistemi", groupBy("os"), [], ["OS", ...heads], false);
  body += await table(db, "Arayüz", groupBy("client"), [], ["Arayüz", ...heads], false);
  body += await table(db, "Sürüm", groupBy("version"), [], ["Sürüm", ...heads], false);
  body += await table(
    db,
    "Kategori dağılımı (tüm taramalar)",
    "SELECT SUM(duplicate_bytes), SUM(junk_bytes), SUM(old_download_bytes), SUM(large_bytes) FROM events WHERE kind='scan'",
    [],
    ["Kopyalar (boyut)", "Gereksiz (boyut)", "Eski indirmeler (boyut)", "Büyük dosyalar (boyut)"],
    false,
  );
  body += await table(
    db,
    "Bileşen markaları (son bilinen donanım)",
    `SELECT json_extract(s.value, '$.kind'), json_extract(s.value, '$.brand'), COUNT(DISTINCT h.install_id)
     FROM hardware h, json_each(h.specs) s WHERE json_extract(s.value, '$.brand') != ''
     GROUP BY 1, 2 ORDER BY 1, 3 DESC`,
    [],
    ["Bileşen", "Marka", "Kullanıcı"],
    false,
  );
  body += await table(
    db,
    "Son 30 gün",
    `SELECT date(ts,'unixepoch'), COUNT(DISTINCT install_id), SUM(kind='scan'), SUM(kind='clean'),
            SUM(CASE WHEN kind='clean' THEN bytes ELSE 0 END)
     FROM events WHERE ts >= ?1 GROUP BY 1 ORDER BY 1 DESC`,
    [month],
    ["Gün", "Aktif kullanıcı", "Tarama", "Temizleme", "Kazanılan (boyut)"],
    false,
  );
  body += await table(
    db,
    "Kullanıcılar (son görülen, ilk 500)",
    `SELECT install_id, MAX(os),
            (SELECT json_extract(s.value, '$.name') FROM hardware h, json_each(h.specs) s
             WHERE h.install_id = events.install_id AND json_extract(s.value, '$.kind') = 'İşlemci'),
            group_concat(DISTINCT client), MAX(version),
            datetime(MIN(ts),'unixepoch'), datetime(MAX(ts),'unixepoch'),
            SUM(kind='open'), SUM(kind='scan'), SUM(kind='clean'), SUM(CASE WHEN kind='clean' THEN bytes ELSE 0 END)
     FROM events GROUP BY install_id ORDER BY MAX(ts) DESC LIMIT 500`,
    [],
    ["Kurulum ID", "OS", "İşlemci", "Arayüz", "Sürüm", "İlk görülme (UTC)", "Son görülme (UTC)", "Açılış", "Tarama", "Temizleme", "Kazanılan (boyut)"],
    true,
  );
  return layout("deskMonk by pelamx — Admin", body);
}

async function userPage(db: D1Database, id: string): Promise<string> {
  let body = `<a href="/admin">← Genel bakış</a><h1>Kurulum ${esc(id)}</h1>`;
  body += await cards(
    db,
    `SELECT SUM(kind='open'), SUM(kind='scan'), SUM(kind='clean'),
            CAST(AVG(CASE WHEN kind='scan' THEN files END) AS INTEGER), CAST(AVG(CASE WHEN kind='scan' THEN bytes END) AS INTEGER),
            SUM(CASE WHEN kind='clean' THEN bytes END), SUM(CASE WHEN kind='clean' THEN files END)
     FROM events WHERE install_id = ?1`,
    [id],
    ["Uygulama açılışı", "Tarama", "Temizleme", "Ort. taranan dosya", "Ort. bulunan alan (boyut)", "Kazanılan alan (boyut)", "Silinen dosya"],
  );
  body += await hardwareBlock(db, id);
  body += await table(
    db,
    "Tüm olaylar",
    `SELECT datetime(ts,'unixepoch'), kind, client, os, version, files, bytes, duplicate_bytes, junk_bytes, old_download_bytes, large_bytes
     FROM events WHERE install_id = ?1 ORDER BY ts DESC LIMIT 1000`,
    [id],
    ["Zaman (UTC)", "Olay", "Arayüz", "OS", "Sürüm", "Dosya", "Toplam (boyut)", "Kopyalar (boyut)", "Gereksiz (boyut)", "Eski indirmeler (boyut)", "Büyük (boyut)"],
    false,
  );
  return layout(`Kurulum ${esc(id)}`, body);
}

/** The installation's latest hardware as a terminal-style listing. */
async function hardwareBlock(db: D1Database, id: string): Promise<string> {
  const row = await db
    .prepare("SELECT datetime(ts,'unixepoch') AS ts, specs FROM hardware WHERE install_id = ?1")
    .bind(id)
    .first<{ ts: string; specs: string }>();
  if (!row) return "<h2>Donanım</h2><div class=muted>Henüz donanım bilgisi gelmedi.</div>";
  return hardwareHtml(row.ts, row.specs);
}

export function hardwareHtml(ts: string, specsJson: string): string {
  let specs: Record<string, string>[] = [];
  try {
    specs = JSON.parse(specsJson);
  } catch {}
  let out = `<h2>Donanım</h2><pre class=term><span class=dim>root@deskmonk:~$ ./hwinfo --brands  # ${esc(ts)} UTC</span>\n`;
  for (const s of specs) {
    const field = (k: string) => esc(s[k] ?? "");
    const brand = field("brand") ? `<span class=b>[${field("brand").toUpperCase()}]</span> ` : "";
    out += `<span class=k>${field("kind").replace(/i/g, "İ").toUpperCase()}</span> ${brand}${field("name")}\n`;
    if (field("detail")) out += `<span class=dim>${" ".repeat(12)} └─ ${field("detail")}</span>\n`;
  }
  return out + "<span class=dim>root@deskmonk:~$</span> <span class=cur>█</span></pre>";
}
