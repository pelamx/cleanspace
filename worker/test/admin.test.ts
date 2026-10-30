//! The admin panel is a pile of hand-written SQL that nothing ever ran: a typo in any of it would
//! have shown up as a 500 in production and nowhere else. These render the real page against the
//! real schema, once empty and once with data in every table it reads.

import { expect, test } from "vitest";
import worker from "../src/index";
import { fakeD1 } from "./d1";

// Workers provides crypto.subtle.timingSafeEqual; Node does not. The comparison is the worker's
// own, so supply the missing primitive here rather than weaken production code for the tests.
// Lengths are already checked by the caller, so this only has to be constant time for equal ones.
const subtle = crypto.subtle as unknown as Record<string, unknown>;
subtle.timingSafeEqual ??= (a: ArrayBufferView, b: ArrayBufferView) => {
  const [x, y] = [new Uint8Array(a.buffer, a.byteOffset, a.byteLength), new Uint8Array(b.buffer, b.byteOffset, b.byteLength)];
  return x.length === y.length && x.reduce((acc, v, i) => acc | (v ^ y[i]), 0) === 0;
};

const TOKEN = "admin-token-long-enough";

async function admin(db: D1Database, path = "/admin"): Promise<Response> {
  const env = { DB: db, ADMIN_TOKEN: TOKEN } as never;
  return worker.fetch(new Request(`https://x${path}`, { headers: { Authorization: `Basic ${btoa(`admin:${TOKEN}`)}` } }), env);
}

test("the admin panel is behind the token", async () => {
  const db = fakeD1();
  const env = { DB: db, ADMIN_TOKEN: TOKEN } as never;
  expect((await worker.fetch(new Request("https://x/admin"), env)).status).toBe(401);
  expect((await worker.fetch(new Request("https://x/admin"), { DB: db, ADMIN_TOKEN: "short" } as never)).status).toBe(500);
  expect((await admin(db)).status).toBe(200);
});

test("every query on the overview runs, with data and without", async () => {
  const db = fakeD1();
  // Empty database: the panel must still render rather than throw on a null.
  const empty = await admin(db);
  expect(empty.status).toBe(200);
  expect(await empty.text()).toContain("Pro");

  const now = Math.floor(Date.now() / 1000);
  const ev = (install: string, ts: number, kind: string, bytes: number) =>
    db
      .prepare(
        `INSERT INTO events(ts, install_id, version, os, client, kind, files, bytes,
           duplicate_bytes, junk_bytes, old_download_bytes, large_bytes)
         VALUES (?1, ?2, '0.3.0', 'windows', 'gui', ?3, 3, ?4, 1, 2, 3, 4)`,
      )
      .bind(ts, install, kind, bytes)
      .run();
  // One installation seen a minute ago, one a week ago: the live and the 30-day numbers differ.
  await ev("live-1", now - 60, "scan", 2048);
  await ev("live-1", now - 30, "clean", 1024);
  await ev("old-1", now - 8 * 86400, "open", 0);
  await db
    .prepare("INSERT INTO hardware(install_id, ts, specs) VALUES ('live-1', ?1, ?2)")
    .bind(now, JSON.stringify([{ kind: "İşlemci", brand: "Apple", name: "Apple M5", detail: "" }]))
    .run();
  await db
    .prepare(
      `INSERT INTO subscriptions(id, customer_id, status, period_end, license_key, install_id, txn, event_at)
       VALUES ('sub_1','ctm_1','active',?1,'DM-AAAA-BBBB-CCCC','live-1','txn_x','2026-10-30T00:00:00Z')`,
    )
    .bind(now + 30 * 86400)
    .run();
  await db.prepare("INSERT INTO activations(license_key, install_id, ts) VALUES ('DM-AAAA-BBBB-CCCC','live-1',?1)").bind(now).run();

  const html = await (await admin(db)).text();
  expect(html).toContain("Şu an açık");
  expect(html).toContain("ctm_1"); // the subscription reached the Pro table
  expect(html).toContain("Apple"); // hardware brands still work
  expect(html).toContain("live-1");

  // And the per-installation page, which has its own set of queries.
  const user = await admin(db, "/admin/user/live-1");
  expect(user.status).toBe(200);
  expect(await user.text()).toContain("live-1");
});
