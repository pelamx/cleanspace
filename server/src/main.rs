//! DeskMonk telemetry server + admin panel.
//!
//! Env: DESKMONK_ADMIN_TOKEN (required), DESKMONK_ADDR (default 0.0.0.0:8080), DESKMONK_DB (default deskmonk.db).
//! POST /api/events        ← apps send JSON events here
//! GET  /admin             overview (HTTP Basic auth: user "admin", password = admin token)
//! GET  /admin/user/<id>   one installation's full history and hardware

use base64::Engine;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, ToSql, params};
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};
use tiny_http::{Header, Method, Request, Response, Server};

// Room for an "open" event's hardware list.
const MAX_BODY: u64 = 16384;
const MAX_SPECS: usize = 40;
const MAX_SPEC_TEXT: usize = 200;
const SIZE_FIELDS: [&str; 5] = [
    "bytes",
    "duplicate_bytes",
    "junk_bytes",
    "old_download_bytes",
    "large_bytes",
];

fn main() {
    let token = std::env::var("DESKMONK_ADMIN_TOKEN").unwrap_or_default();
    if token.len() < 12 {
        eprintln!("DESKMONK_ADMIN_TOKEN must be set (at least 12 characters)");
        std::process::exit(1);
    }
    let expected_auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("admin:{token}"))
    );
    let addr = std::env::var("DESKMONK_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let db = open_db(&std::env::var("DESKMONK_DB").unwrap_or_else(|_| "deskmonk.db".into()))
        .expect("open database");
    let server = Server::http(&addr).expect("bind address");
    println!("DeskMonk server listening on http://{addr}  (admin: /admin)");

    // shortcut: single-threaded request loop; fine for thousands of events/minute, put it behind a queue or async runtime beyond that.
    for req in server.incoming_requests() {
        if let Err(e) = handle(req, &db, &expected_auth) {
            eprintln!("request error: {e}");
        }
    }
}

fn open_db(path: &str) -> rusqlite::Result<Connection> {
    let db = Connection::open(path)?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS events(
            id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, install_id TEXT NOT NULL, version TEXT NOT NULL,
            os TEXT NOT NULL, client TEXT NOT NULL, kind TEXT NOT NULL, files INTEGER NOT NULL, bytes INTEGER NOT NULL,
            duplicate_bytes INTEGER NOT NULL, junk_bytes INTEGER NOT NULL, old_download_bytes INTEGER NOT NULL, large_bytes INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS events_install ON events(install_id, ts);
         CREATE INDEX IF NOT EXISTS events_ts ON events(ts);
         CREATE TABLE IF NOT EXISTS hardware(install_id TEXT PRIMARY KEY, ts INTEGER NOT NULL, specs TEXT NOT NULL);",
    )?;
    Ok(db)
}

fn handle(mut req: Request, db: &Connection, expected_auth: &str) -> std::io::Result<()> {
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("");

    if req.method() == &Method::Post && path == "/api/events" {
        let mut body = String::new();
        if req
            .as_reader()
            .take(MAX_BODY + 1)
            .read_to_string(&mut body)
            .is_err()
            || body.len() as u64 > MAX_BODY
        {
            return req.respond(
                Response::from_string("body too large or not utf-8").with_status_code(400),
            );
        }
        return match parse_event(&body).map(|ev| insert(db, &ev).map_err(|e| e.to_string())) {
            Ok(Ok(())) => req.respond(Response::empty(204)),
            Ok(Err(e)) => {
                eprintln!("db error: {e}");
                req.respond(Response::empty(500))
            }
            Err(e) => req.respond(Response::from_string(e).with_status_code(400)),
        };
    }

    if req.method() != &Method::Get || !(path == "/admin" || path.starts_with("/admin/")) {
        return req.respond(Response::empty(404));
    }
    let auth = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default();
    if !constant_time_eq(auth.as_bytes(), expected_auth.as_bytes()) {
        let challenge =
            Header::from_bytes("WWW-Authenticate", "Basic realm=\"DeskMonk admin\"").unwrap();
        return req.respond(Response::empty(401).with_header(challenge));
    }
    let page = match path.strip_prefix("/admin/user/") {
        Some(id) => user_page(db, id),
        None => overview_page(db),
    };
    let html = Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap();
    match page {
        Ok(body) => req.respond(Response::from_string(body).with_header(html)),
        Err(e) => {
            eprintln!("db error: {e}");
            req.respond(Response::from_string("database error").with_status_code(500))
        }
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

struct Event {
    install_id: String,
    version: String,
    os: String,
    client: String,
    kind: String,
    files: i64,
    sizes: [i64; 5],
    /// Latest hardware list (re-serialized validated JSON), sent with "open" events.
    specs: Option<String>,
}

/// Validates untrusted input from the apps.
fn parse_event(body: &str) -> Result<Event, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("bad json: {e}"))?;
    let text = |key: &str, max: usize| -> Result<String, String> {
        let s = v[key].as_str().ok_or(format!("missing {key}"))?;
        let ok = !s.is_empty()
            && s.len() <= max
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-._".contains(c));
        if ok {
            Ok(s.to_string())
        } else {
            Err(format!("invalid {key}"))
        }
    };
    // Missing counts are 0 (clean events have no category breakdown); present ones must be non-negative integers.
    let count = |key: &str| -> Result<i64, String> {
        match &v[key] {
            serde_json::Value::Null => Ok(0),
            n => n
                .as_i64()
                .filter(|n| *n >= 0)
                .ok_or(format!("invalid {key}")),
        }
    };
    let kind = text("kind", 16)?;
    if !["scan", "clean", "open"].contains(&kind.as_str()) {
        return Err("invalid kind".into());
    }
    let mut sizes = [0; 5];
    for (slot, key) in sizes.iter_mut().zip(SIZE_FIELDS) {
        *slot = count(key)?;
    }
    // Free text from the OS (model names); shown escaped, so only length and control characters are limited.
    let specs = match &v["specs"] {
        serde_json::Value::Null => None,
        serde_json::Value::Array(list) if kind == "open" && list.len() <= MAX_SPECS => {
            let mut clean = Vec::new();
            for spec in list {
                let mut obj = serde_json::Map::new();
                for key in ["kind", "brand", "name", "detail"] {
                    let s = spec[key].as_str().ok_or("invalid specs")?;
                    if s.chars().count() > MAX_SPEC_TEXT || s.chars().any(char::is_control) {
                        return Err("invalid specs".into());
                    }
                    obj.insert(key.into(), s.into());
                }
                clean.push(serde_json::Value::Object(obj));
            }
            Some(serde_json::Value::Array(clean).to_string())
        }
        _ => return Err("invalid specs".into()),
    };
    Ok(Event {
        install_id: text("install_id", 64)?,
        version: text("version", 32)?,
        os: text("os", 32)?,
        client: text("client", 16)?,
        kind,
        files: count("files")?,
        sizes,
        specs,
    })
}

fn insert(db: &Connection, e: &Event) -> rusqlite::Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let [b, d, j, o, l] = e.sizes;
    db.execute(
        "INSERT INTO events(ts, install_id, version, os, client, kind, files, bytes, duplicate_bytes, junk_bytes, old_download_bytes, large_bytes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![now, e.install_id, e.version, e.os, e.client, e.kind, e.files, b, d, j, o, l],
    )?;
    if let Some(specs) = &e.specs {
        db.execute(
            "INSERT INTO hardware(install_id, ts, specs) VALUES (?1, ?2, ?3)
             ON CONFLICT(install_id) DO UPDATE SET ts = excluded.ts, specs = excluded.specs",
            params![e.install_id, now, specs],
        )?;
    }
    Ok(())
}

// ---------- admin HTML ----------

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn human_size(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let (mut size, mut unit) = (bytes as f64, 0);
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Renders a query as an HTML table. Columns named in `heads` ending with " (boyut)" are shown as sizes;
/// with `link`, the first column is an install id linking to its detail page.
fn table(
    db: &Connection,
    title: &str,
    sql: &str,
    args: &[&dyn ToSql],
    heads: &[&str],
    link: bool,
) -> rusqlite::Result<String> {
    let mut out = format!("<h2>{}</h2><div class=scroll><table><tr>", esc(title));
    for h in heads {
        out += &format!("<th>{}</th>", esc(h.trim_end_matches(" (boyut)")));
    }
    out += "</tr>";
    let mut stmt = db.prepare(sql)?;
    let mut rows = stmt.query(args)?;
    let mut empty = true;
    while let Some(row) = rows.next()? {
        empty = false;
        out += "<tr>";
        for (i, h) in heads.iter().enumerate() {
            let cell = match row.get_ref(i)? {
                ValueRef::Null => String::new(),
                ValueRef::Integer(n) if h.ends_with("(boyut)") => human_size(n),
                ValueRef::Integer(n) => n.to_string(),
                ValueRef::Real(f) => format!("{f:.1}"),
                ValueRef::Text(t) | ValueRef::Blob(t) => esc(&String::from_utf8_lossy(t)),
            };
            let num = matches!(row.get_ref(i)?, ValueRef::Integer(_) | ValueRef::Real(_));
            out += &match (link && i == 0, num) {
                (true, _) => format!("<td><a href=\"/admin/user/{cell}\">{cell}</a></td>"),
                (false, true) => format!("<td class=n>{cell}</td>"),
                (false, false) => format!("<td>{cell}</td>"),
            };
        }
        out += "</tr>";
    }
    if empty {
        out += &format!("<tr><td colspan={}>Henüz veri yok</td></tr>", heads.len());
    }
    Ok(out + "</table></div>")
}

fn layout(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=tr><head><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\">
<title>{title}</title><style>
:root{{color-scheme:light dark;--bg:#f7f7f8;--fg:#1d1d1f;--card:#fff;--line:#e3e3e6;--muted:#6b6b70;--accent:#2563eb}}
@media(prefers-color-scheme:dark){{:root{{--bg:#151517;--fg:#ececee;--card:#1f1f22;--line:#333338;--muted:#9a9aa0;--accent:#6ea0ff}}}}
body{{margin:0;padding:24px 16px;background:var(--bg);color:var(--fg);font:14px/1.45 system-ui,sans-serif}}
main{{max-width:1200px;margin:auto}}a{{color:var(--accent)}}h1{{margin:0 0 4px}}h2{{margin:28px 0 8px;font-size:16px}}
.muted{{color:var(--muted)}}.cards{{display:grid;grid-template-columns:repeat(auto-fill,minmax(170px,1fr));gap:12px;margin-top:16px}}
.card{{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:12px 14px}}
.card b{{display:block;font-size:22px;font-variant-numeric:tabular-nums}}.card span{{color:var(--muted);font-size:12px}}
.scroll{{overflow-x:auto;background:var(--card);border:1px solid var(--line);border-radius:10px}}
table{{border-collapse:collapse;width:100%}}th,td{{padding:7px 10px;border-bottom:1px solid var(--line);text-align:left;white-space:nowrap}}
th{{font-size:12px;color:var(--muted);font-weight:600}}td.n{{text-align:right;font-variant-numeric:tabular-nums}}tr:last-child td{{border-bottom:0}}
.term{{background:#070b08;color:#39ff14;border:1px solid #1e5c2b;border-radius:10px;padding:16px 18px;overflow-x:auto;
font:13px/1.6 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;text-shadow:0 0 6px rgba(57,255,20,.35)}}
.term .dim{{color:#1f9e45}}.term .k{{color:#1f9e45;display:inline-block;min-width:12ch}}.term .b{{font-weight:700}}
.term .cur{{animation:blink 1s steps(1) infinite}}@keyframes blink{{50%{{opacity:0}}}}
</style></head><body><main>{body}</main></body></html>"
    )
}

fn cards(
    db: &Connection,
    sql: &str,
    args: &[&dyn ToSql],
    labels: &[&str],
) -> rusqlite::Result<String> {
    db.query_row(sql, args, |row| {
        let mut out = String::from("<div class=cards>");
        for (i, label) in labels.iter().enumerate() {
            let n: i64 = row.get::<_, Option<i64>>(i)?.unwrap_or(0);
            let value = if label.ends_with("(boyut)") {
                human_size(n)
            } else {
                n.to_string()
            };
            out += &format!(
                "<div class=card><b>{value}</b><span>{}</span></div>",
                esc(label.trim_end_matches(" (boyut)"))
            );
        }
        Ok(out + "</div>")
    })
}

const DAY: i64 = 86400;

fn overview_page(db: &Connection) -> rusqlite::Result<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let (week, month) = (now - 7 * DAY, now - 30 * DAY);
    let mut body = String::from(
        "<h1>deskMonk by pelamx — Admin</h1><div class=muted>Anonim kullanım istatistikleri</div>",
    );
    body += &cards(
        db,
        "SELECT COUNT(DISTINCT install_id),
                COUNT(DISTINCT CASE WHEN ts >= ?1 THEN install_id END),
                COUNT(DISTINCT CASE WHEN ts >= ?2 THEN install_id END),
                SUM(kind='scan'), SUM(kind='clean'),
                SUM(CASE WHEN kind='scan' THEN bytes END), SUM(CASE WHEN kind='clean' THEN bytes END),
                SUM(CASE WHEN kind='clean' THEN files END), SUM(kind='open')
         FROM events",
        &[&week, &month],
        &["Toplam kullanıcı", "Aktif (7 gün)", "Aktif (30 gün)", "Tarama", "Temizleme", "Bulunan alan (boyut)", "Kazanılan alan (boyut)", "Silinen dosya", "Uygulama açılışı"],
    )?;
    let group_by = |col: &str| {
        format!(
            "SELECT {col}, COUNT(DISTINCT install_id), SUM(kind='scan'), SUM(kind='clean'),
                    SUM(CASE WHEN kind='clean' THEN bytes ELSE 0 END)
             FROM events GROUP BY {col} ORDER BY 2 DESC"
        )
    };
    let heads = ["", "Kullanıcı", "Tarama", "Temizleme", "Kazanılan (boyut)"];
    body += &table(
        db,
        "İşletim sistemi",
        &group_by("os"),
        &[],
        &["OS", heads[1], heads[2], heads[3], heads[4]],
        false,
    )?;
    body += &table(
        db,
        "Arayüz",
        &group_by("client"),
        &[],
        &["Arayüz", heads[1], heads[2], heads[3], heads[4]],
        false,
    )?;
    body += &table(
        db,
        "Sürüm",
        &group_by("version"),
        &[],
        &["Sürüm", heads[1], heads[2], heads[3], heads[4]],
        false,
    )?;
    body += &table(
        db,
        "Kategori dağılımı (tüm taramalar)",
        "SELECT SUM(duplicate_bytes), SUM(junk_bytes), SUM(old_download_bytes), SUM(large_bytes) FROM events WHERE kind='scan'",
        &[],
        &[
            "Kopyalar (boyut)",
            "Gereksiz (boyut)",
            "Eski indirmeler (boyut)",
            "Büyük dosyalar (boyut)",
        ],
        false,
    )?;
    body += &table(
        db,
        "Bileşen markaları (son bilinen donanım)",
        "SELECT json_extract(s.value, '$.kind'), json_extract(s.value, '$.brand'), COUNT(DISTINCT h.install_id)
         FROM hardware h, json_each(h.specs) s WHERE json_extract(s.value, '$.brand') != ''
         GROUP BY 1, 2 ORDER BY 1, 3 DESC",
        &[],
        &["Bileşen", "Marka", "Kullanıcı"],
        false,
    )?;
    body += &table(
        db,
        "Son 30 gün",
        "SELECT date(ts,'unixepoch'), COUNT(DISTINCT install_id), SUM(kind='scan'), SUM(kind='clean'),
                SUM(CASE WHEN kind='clean' THEN bytes ELSE 0 END)
         FROM events WHERE ts >= ?1 GROUP BY 1 ORDER BY 1 DESC",
        &[&month],
        &["Gün", "Aktif kullanıcı", "Tarama", "Temizleme", "Kazanılan (boyut)"],
        false,
    )?;
    body += &table(
        db,
        "Kullanıcılar (son görülen, ilk 500)",
        "SELECT install_id, MAX(os),
                (SELECT json_extract(s.value, '$.name') FROM hardware h, json_each(h.specs) s
                 WHERE h.install_id = events.install_id AND json_extract(s.value, '$.kind') = 'İşlemci'),
                group_concat(DISTINCT client), MAX(version),
                datetime(MIN(ts),'unixepoch'), datetime(MAX(ts),'unixepoch'),
                SUM(kind='open'), SUM(kind='scan'), SUM(kind='clean'), SUM(CASE WHEN kind='clean' THEN bytes ELSE 0 END)
         FROM events GROUP BY install_id ORDER BY MAX(ts) DESC LIMIT 500",
        &[],
        &["Kurulum ID", "OS", "İşlemci", "Arayüz", "Sürüm", "İlk görülme (UTC)", "Son görülme (UTC)", "Açılış", "Tarama", "Temizleme", "Kazanılan (boyut)"],
        true,
    )?;
    Ok(layout("deskMonk by pelamx — Admin", &body))
}

fn user_page(db: &Connection, id: &str) -> rusqlite::Result<String> {
    let mut body = format!(
        "<a href=\"/admin\">← Genel bakış</a><h1>Kurulum {}</h1>",
        esc(id)
    );
    body += &cards(
        db,
        "SELECT SUM(kind='open'), SUM(kind='scan'), SUM(kind='clean'),
                CAST(AVG(CASE WHEN kind='scan' THEN files END) AS INTEGER), CAST(AVG(CASE WHEN kind='scan' THEN bytes END) AS INTEGER),
                SUM(CASE WHEN kind='clean' THEN bytes END), SUM(CASE WHEN kind='clean' THEN files END)
         FROM events WHERE install_id = ?1",
        &[&id],
        &["Uygulama açılışı", "Tarama", "Temizleme", "Ort. taranan dosya", "Ort. bulunan alan (boyut)", "Kazanılan alan (boyut)", "Silinen dosya"],
    )?;
    body += &hardware_block(db, id)?;
    body += &table(
        db,
        "Tüm olaylar",
        "SELECT datetime(ts,'unixepoch'), kind, client, os, version, files, bytes, duplicate_bytes, junk_bytes, old_download_bytes, large_bytes
         FROM events WHERE install_id = ?1 ORDER BY ts DESC LIMIT 1000",
        &[&id],
        &["Zaman (UTC)", "Olay", "Arayüz", "OS", "Sürüm", "Dosya", "Toplam (boyut)", "Kopyalar (boyut)", "Gereksiz (boyut)", "Eski indirmeler (boyut)", "Büyük (boyut)"],
        false,
    )?;
    Ok(layout(&format!("Kurulum {}", esc(id)), &body))
}

/// The installation's latest hardware as a terminal-style listing.
fn hardware_block(db: &Connection, id: &str) -> rusqlite::Result<String> {
    let row: Option<(String, String)> = db
        .query_row(
            "SELECT datetime(ts,'unixepoch'), specs FROM hardware WHERE install_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;
    let Some((ts, specs)) = row else {
        return Ok("<h2>Donanım</h2><div class=muted>Henüz donanım bilgisi gelmedi.</div>".into());
    };
    let specs: serde_json::Value = serde_json::from_str(&specs).unwrap_or_default();
    let mut out = format!(
        "<h2>Donanım</h2><pre class=term><span class=dim>root@deskmonk:~$ ./hwinfo --brands  # {} UTC</span>\n",
        esc(&ts)
    );
    for s in specs.as_array().into_iter().flatten() {
        let field = |k: &str| esc(s[k].as_str().unwrap_or(""));
        let brand = field("brand");
        let brand = if brand.is_empty() { String::new() } else { format!("<span class=b>[{}]</span> ", brand.to_uppercase()) };
        out += &format!("<span class=k>{}</span> {brand}{}\n", field("kind").replace('i', "İ").to_uppercase(), field("name"));
        if !field("detail").is_empty() {
            out += &format!("<span class=dim>{:12} └─ {}</span>\n", "", field("detail"));
        }
    }
    Ok(out + "<span class=dim>root@deskmonk:~$</span> <span class=cur>█</span></pre>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_and_renders() {
        let good = r#"{"install_id":"abc-123","version":"0.1.0","os":"linux","client":"tui","kind":"scan","files":10,"bytes":2048,"junk_bytes":2048}"#;
        let ev = parse_event(good).unwrap();
        assert_eq!(ev.sizes, [2048, 0, 2048, 0, 0]);
        assert!(parse_event(&good.replace("abc-123", "<script>")).is_err());
        assert!(parse_event(&good.replace("\"scan\"", "\"drop\"")).is_err());
        assert!(parse_event(&good.replace("10", "-1")).is_err());

        let db = open_db(":memory:").unwrap();
        insert(&db, &ev).unwrap();
        assert!(overview_page(&db).unwrap().contains("/admin/user/abc-123"));
        assert!(user_page(&db, "abc-123").unwrap().contains("2.0 KB"));
        assert!(user_page(&db, "<x>").unwrap().contains("&lt;x&gt;"));

        let open = r#"{"install_id":"abc-123","version":"0.2.0","os":"macos","client":"gui","kind":"open",
            "specs":[{"kind":"İşlemci","brand":"Apple","name":"Apple M5 Pro <b>","detail":"15 çekirdek"}]}"#;
        insert(&db, &parse_event(open).unwrap()).unwrap();
        insert(&db, &parse_event(open).unwrap()).unwrap();
        let page = user_page(&db, "abc-123").unwrap();
        assert!(page.contains("[APPLE]") && page.contains("Apple M5 Pro &lt;b&gt;") && !page.contains("Pro <b>"));
        let overview = overview_page(&db).unwrap();
        assert!(overview.contains("<td>Apple M5 Pro &lt;b&gt;</td>"));
        assert!(parse_event(&open.replace("15 çekirdek", "a\\u0007")).is_err());
        assert!(parse_event(&good.replace("\"files\":10", "\"specs\":[]")).is_err());
        assert!(constant_time_eq(b"abc", b"abc") && !constant_time_eq(b"abc", b"abd"));
    }
}
