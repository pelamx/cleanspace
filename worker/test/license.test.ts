//! The money path: a webhook turning into a licence, and the licence turning into Pro. Previously
//! only the pure helpers were covered, so nothing checked the statements that decide who paid.

import { expect, test } from "vitest";
import { handleLicense, MAX_DEVICES, type LicenseEnv } from "../src/license";
import { event, fakeD1, sign } from "./d1";

const SECRET = "whsec_test";

function envWith(over: Partial<LicenseEnv> = {}): LicenseEnv {
  return { DB: fakeD1(), PADDLE_WEBHOOK_SECRET: SECRET, ...over };
}

async function post(env: LicenseEnv, path: string, body: string, headers: Record<string, string> = {}) {
  const res = await handleLicense(new Request(`https://x${path}`, { method: "POST", body, headers }), env, path);
  return res!;
}

/** Delivers a signed webhook the way Paddle would. */
async function deliver(env: LicenseEnv, body: string) {
  const header = await sign(body, SECRET, Math.floor(Date.now() / 1000));
  return post(env, "/api/paddle", body, { "Paddle-Signature": header });
}

async function validate(env: LicenseEnv, key: string, install: string) {
  const res = await post(env, "/api/license/validate", JSON.stringify({ key, install_id: install }));
  return { status: res.status, body: (await res.json()) as { key?: string; pro_until?: number; error?: string } };
}

test("an unsigned or stale webhook changes nothing", async () => {
  const env = envWith();
  expect((await post(env, "/api/paddle", event(), { "Paddle-Signature": "ts=1;h1=00" })).status).toBe(401);
  // Correct HMAC, but older than the replay window.
  const body = event();
  const stale = await sign(body, SECRET, Math.floor(Date.now() / 1000) - 3600);
  expect((await post(env, "/api/paddle", body, { "Paddle-Signature": stale })).status).toBe(401);
  // A real secret must be configured; an empty one must not accept everything.
  const open = envWith({ PADDLE_WEBHOOK_SECRET: "" });
  expect((await deliver(open, event())).status).toBe(401);
  expect((await validate(env, "", "install-1")).body.pro_until).toBe(0);
});

test("a paid subscription becomes a Pro licence", async () => {
  const env = envWith();
  expect((await deliver(env, event())).status).toBe(200);

  // The app asks with no key at all right after checkout, and is matched by installation.
  const first = await validate(env, "", "install-1");
  expect(first.status).toBe(200);
  expect(first.body.key).toMatch(/^DM(-[A-HJ-NP-Z2-9]{4}){3}$/);
  expect(first.body.pro_until).toBe(Math.floor(Date.parse("2026-11-30T00:00:00.000Z") / 1000));

  // And with the key from any other installation.
  expect((await validate(env, first.body.key!, "install-2")).body.pro_until).toBe(first.body.pro_until);
  expect((await validate(env, "DM-AAAA-BBBB-CCCC", "install-9")).status).toBe(404);
  // A malformed key is refused without reaching the database.
  expect((await validate(env, "nonsense", "install-9")).status).toBe(404);
});

test("the licence key survives later events, and an out-of-order one cannot undo Pro", async () => {
  const env = envWith();
  await deliver(env, event());
  const key = (await validate(env, "", "install-1")).body.key!;

  // A renewal arrives: same subscription, later date, further period end.
  await deliver(env, event({ event_type: "subscription.updated", occurred_at: "2026-11-30T00:00:00.000Z" }, { current_billing_period: { ends_at: "2026-12-31T00:00:00.000Z" } }));
  const renewed = await validate(env, key, "install-1");
  expect(renewed.body.key).toBe(key); // the customer's key must never change under them
  expect(renewed.body.pro_until).toBe(Math.floor(Date.parse("2026-12-31T00:00:00.000Z") / 1000));

  // A cancellation that was emitted *earlier* lands late; it must not revoke the newer state.
  await deliver(env, event({ event_type: "subscription.canceled", occurred_at: "2026-10-01T00:00:00.000Z" }, { status: "canceled", current_billing_period: { ends_at: "2026-10-02T00:00:00.000Z" } }));
  expect((await validate(env, key, "install-1")).body.pro_until).toBe(Math.floor(Date.parse("2026-12-31T00:00:00.000Z") / 1000));
});

test("a real cancellation ends Pro", async () => {
  const env = envWith();
  await deliver(env, event());
  const key = (await validate(env, "", "install-1")).body.key!;
  await deliver(env, event({ event_type: "subscription.canceled", occurred_at: "2026-12-01T00:00:00.000Z" }, { status: "canceled" }));
  expect((await validate(env, key, "install-1")).body.pro_until).toBe(0);
});

test("a key stops at the device limit but keeps working on the computers it already has", async () => {
  const env = envWith();
  await deliver(env, event());
  const key = (await validate(env, "", "install-1")).body.key!;
  // install-1 was activated by the webhook itself; fill the rest of the allowance.
  for (let i = 2; i <= MAX_DEVICES; i++) {
    expect((await validate(env, key, `install-${i}`)).status).toBe(200);
  }
  const extra = await validate(env, key, `install-${MAX_DEVICES + 1}`);
  expect(extra.status).toBe(403);
  expect(extra.body.error).toBe("too_many_devices");
  // The refusal must not have cost an existing computer its activation.
  expect((await validate(env, key, "install-1")).status).toBe(200);
  expect((await validate(env, key, `install-${MAX_DEVICES}`)).status).toBe(200);
});

test("checkout refuses to pretend it works before Paddle is configured", async () => {
  const env = envWith();
  const buy = async (e: LicenseEnv) => (await handleLicense(new Request("https://x/buy?install=install-1&plan=monthly"), e, "/buy"))!;
  // This is the state the repository ships in: no price ids, no client token.
  expect((await buy(env)).status).toBe(503);
  expect((await buy(envWith({ PADDLE_PRICE_MONTHLY: "pri_1" }))).status).toBe(503);

  const live = envWith({ PADDLE_PRICE_MONTHLY: "pri_1", PADDLE_CLIENT_TOKEN: "live_tok" });
  const ok = await buy(live);
  expect(ok.status).toBe(200);
  const html = await ok.text();
  expect(html).toContain("pri_1");
  // The install id is reflected into a <script>; it must not be able to close the tag.
  const nasty = await handleLicense(new Request("https://x/buy?install=%3C/script%3E&plan=monthly"), live, "/buy");
  expect(nasty!.status).toBe(400);
});

test("the portal needs a key that exists", async () => {
  const env = envWith();
  expect((await post(env, "/api/license/portal", JSON.stringify({ key: "" }))).status).toBe(404);
  expect((await post(env, "/api/license/portal", JSON.stringify({ key: "DM-AAAA-BBBB-CCCC" }))).status).toBe(404);
});

test("junk input is refused", async () => {
  const env = envWith();
  expect((await post(env, "/api/license/validate", "not json")).status).toBe(400);
  expect((await post(env, "/api/license/validate", JSON.stringify({ key: "", install_id: "has space" }))).status).toBe(400);
  expect((await post(env, "/api/license/validate", JSON.stringify({ key: "", install_id: "x".repeat(65) }))).status).toBe(400);
  expect((await post(env, "/api/license/validate", "x".repeat(2000))).status).toBe(400);
});
