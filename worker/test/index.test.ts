import { expect, test } from "vitest";
import { esc, hardwareHtml, humanSize, parseEvent } from "../src/index";
import { mayActivate, newKey, periodEnd, verifySignature } from "../src/license";

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
