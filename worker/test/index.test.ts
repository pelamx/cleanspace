import { expect, test } from "vitest";
import { compareVersions, esc, hardwareHtml, humanSize, worldMap } from "../src/admin";
import { parseEvent } from "../src/index";
import { inCidr, mayActivate, newKey, periodEnd, planOf, verifySignature } from "../src/license";
import { buyPage, js, pickLang } from "../src/pages";
import { LANGS, USD, money, word } from "../src/i18n";

test("validates events the way server/ does", () => {
  const good = `{"install_id":"abc-123","version":"0.1.0","os":"linux","client":"tui","kind":"scan","files":10,"bytes":2048,"junk_bytes":2048}`;
  expect(parseEvent(good).sizes).toEqual([2048, 0, 2048, 0, 0]);
  expect(() => parseEvent(good.replace("abc-123", "<script>"))).toThrow();
  expect(() => parseEvent(good.replace('"scan"', '"drop"'))).toThrow();
  expect(() => parseEvent(good.replace("10", "-1"))).toThrow();
  expect(() => parseEvent(good.replace('"files":10', '"specs":[]'))).toThrow();

  const open = `{"install_id":"abc-123","version":"0.2.0","os":"macos","client":"gui","kind":"open",
    "specs":[{"kind":"İşlemci","brand":"Apple","name":"Apple M5 Pro <b>","detail":"15 çekirdek"}]}`;
  const ev = parseEvent(open);
  expect(() => parseEvent(open.replace("15 çekirdek", "a\\u0007"))).toThrow();
  const html = hardwareHtml("2026-10-29 10:00:00", ev.specs!);
  expect(html).toContain("[APPLE]");
  expect(html).toContain("Apple M5 Pro &lt;b&gt;");
  expect(html).not.toContain("Pro <b>");
});

test("events from 0.4: features, plan and store", () => {
  const vault = `{"install_id":"abc-123","version":"0.4.0","os":"macos","client":"gui","kind":"vault","detail":"encrypt","plan":"free","channel":"appstore"}`;
  expect(parseEvent(vault)).toMatchObject({ kind: "vault", detail: "encrypt", plan: "free", channel: "appstore" });
  expect(parseEvent(vault.replace(',"plan":"free","channel":"appstore"', "")).plan).toBeNull();
  expect(() => parseEvent(vault.replace('"free"', '"gold"'))).toThrow();
  expect(() => parseEvent(vault.replace('"appstore"', '"steam"'))).toThrow();
  expect(() => parseEvent(vault.replace('"encrypt"', '"<b>"'))).toThrow();
});

test("only deskMonk's Paddle prices make a license", () => {
  const env = { DB: null as any, PADDLE_PRICE_MONTHLY: "pri_m", PADDLE_PRICE_YEARLY: "pri_y" };
  expect(planOf({ items: [{ price: { id: "pri_y" } }] }, env)).toBe("yearly");
  expect(planOf({ items: [{ price: { id: "pri_m" } }] }, env)).toBe("monthly");
  // Another product sold from the same Paddle account.
  expect(planOf({ items: [{ price: { id: "pri_other" } }] }, env)).toBeNull();
  expect(planOf({}, { DB: null as any })).toBeNull();
});

test("versions sort newest first", () => {
  expect(["0.3.0", "0.10.0", "0.9.1"].sort(compareVersions)).toEqual(["0.10.0", "0.9.1", "0.3.0"]);
});

test("formats", () => {
  expect(humanSize(2048)).toBe("2.0 KB");
  expect(humanSize(5)).toBe("5 B");
  expect(esc("<x>")).toBe("&lt;x&gt;");
});

test("Paddle webhook signatures", async () => {
  const body = '{"event_type":"subscription.created"}';
  const sign = async (ts: number, secret: string) => {
    const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
    const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(`${ts}:${body}`)));
    return [...mac].map((b) => b.toString(16).padStart(2, "0")).join("");
  };
  const now = 1_800_000_000;
  const good = `ts=${now};h1=${await sign(now, "whsec")}`;
  expect(await verifySignature(good, body, "whsec", now + 10)).toBe(true);
  expect(await verifySignature(`ts=${now};h1=00;h1=${await sign(now, "whsec")}`, body, "whsec", now)).toBe(true);
  expect(await verifySignature(good, body + " ", "whsec", now)).toBe(false);
  expect(await verifySignature(good, body, "other", now)).toBe(false);
  expect(await verifySignature(good, body, "whsec", now + 1000)).toBe(false);
  expect(await verifySignature(`ts=${now}`, body, "whsec", now)).toBe(false);
});

test("license rules", () => {
  const ends_at = "2026-12-01T00:00:00Z";
  expect(periodEnd({ status: "active", current_billing_period: { ends_at } })).toBe(Date.parse(ends_at) / 1000);
  expect(periodEnd({ status: "past_due", current_billing_period: { ends_at } })).toBe(Date.parse(ends_at) / 1000);
  expect(periodEnd({ status: "canceled", current_billing_period: null })).toBe(0);
  expect(periodEnd({ status: "paused", current_billing_period: { ends_at } })).toBe(0);
  expect(newKey()).toMatch(/^DM(-[A-HJ-NP-Z2-9]{4}){3}$/);
  expect(mayActivate(["a", "b"], "c")).toBe(true);
  expect(mayActivate(["a", "b", "c"], "d")).toBe(false);
  expect(mayActivate(["a", "b", "c"], "b")).toBe(true);
});

test("buyer pages: language, unconfigured checkout, escaping", async () => {
  const req = (url: string, headers: Record<string, string> = {}) => new Request(url, { headers });
  expect(pickLang(req("https://deskmonk.app/buy?lang=es", { Cookie: "lang=tr" }))).toBe("es");
  expect(pickLang(req("https://deskmonk.app/buy", { Cookie: "a=1; lang=tr" }))).toBe("tr");
  expect(pickLang(req("https://deskmonk.app/buy", { "Accept-Language": "sw-KE,tr;q=0.8" }))).toBe("tr");
  expect(pickLang(req("https://deskmonk.app/buy?lang=xx"))).toBe("en");

  const opts = { configured: false, plan: "yearly" as const, install: "", token: "", sandbox: true, prices: { monthly: "", yearly: "" }, country: null };
  const soon = buyPage(req("https://deskmonk.app/buy"), "tr", opts);
  expect(soon.status).toBe(503);
  expect(await soon.text()).toContain("Ödeme yakında açılıyor");

  const live = await buyPage(req("https://deskmonk.app/buy"), "en", { ...opts, configured: true, token: "</script><b>", prices: { monthly: "pri_m", yearly: "pri_y" } }).text();
  expect(live).not.toContain("</script><b>");
  expect(live).toContain('value="yearly" checked');
  expect(js("</script>")).toBe('"\\u003c/script>"');
});

test("language: chosen, then country, then browser; prices filled in", () => {
  const at = (url: string, country: string | null, headers: Record<string, string> = {}) =>
    Object.assign(new Request(url, { headers }), country ? { cf: { country } } : {});
  expect(pickLang(at("https://deskmonk.app/", "DE", { "Accept-Language": "en-US" }))).toBe("de");
  expect(pickLang(at("https://deskmonk.app/", "BR"))).toBe("pt");
  expect(pickLang(at("https://deskmonk.app/", "HK"))).toBe("zh-TW");
  expect(pickLang(at("https://deskmonk.app/", "DE", { Cookie: "lang=ja" }))).toBe("ja");
  expect(pickLang(at("https://deskmonk.app/?lang=ar", "DE", { Cookie: "lang=ja" }))).toBe("ar");
  expect(pickLang(at("https://deskmonk.app/", "US", { "Accept-Language": "nn-NO,en;q=0.5" }))).toBe("nb");
  expect(pickLang(at("https://deskmonk.app/", "XX", { "Accept-Language": "zh-Hant-TW" }))).toBe("zh-TW");
  expect(pickLang(at("https://deskmonk.app/", null))).toBe("en");


  expect(word("en", "meta.pdesc")).toContain("$2.50 a month or $25 a year");
  expect(money("de", USD.monthly)).toMatch(/2,50\s\$/);
  // Every language has every key, so no page shows English by accident.
  for (const lang of LANGS) expect(word(lang, "hero.title"), lang).not.toBe("");
});

test("world map puts a dot on each known country, none for unknown", () => {
  const svg = worldMap([{ country: "TR", today: 3 }, { country: "?", today: 1 }]);
  expect(svg.match(/class="spot"/g)).toHaveLength(1);
  expect(svg).toContain("Türkiye: 3");
  expect(svg).toContain('class="land" d="M');
});

test("webhook sender check: IPv4 inside Paddle's CIDR blocks only", () => {
  expect(inCidr("34.237.3.244", "34.237.3.244/32")).toBe(true);
  expect(inCidr("34.237.3.245", "34.237.3.244/32")).toBe(false);
  expect(inCidr("10.1.2.3", "10.0.0.0/8")).toBe(true);
  expect(inCidr("11.0.0.1", "10.0.0.0/8")).toBe(false);
  expect(inCidr("", "34.237.3.244/32")).toBe(false);
  expect(inCidr("2001:db8::1", "34.237.3.244/32")).toBe(false);
  expect(inCidr("34.237.3.244", "34.237.3.244/33")).toBe(false);
});
