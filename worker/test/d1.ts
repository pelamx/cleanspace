//! The slice of D1 that the worker uses, backed by real SQLite. The interesting parts of the
//! licence code are the statements themselves — the ON CONFLICT guard that keeps an out-of-order
//! webhook from overwriting newer state, and the activation cap — so a hand-written fake that
//! only pretends to run SQL would test nothing. node:sqlite ships with Node, so this needs no
//! native dependency and no workerd.

import { DatabaseSync } from "node:sqlite";
import { readFileSync } from "node:fs";

export function fakeD1(): D1Database {
  const db = new DatabaseSync(":memory:");
  db.exec(readFileSync(new URL("../schema.sql", import.meta.url), "utf8"));
  const prepare = (sql: string) => {
    let args: unknown[] = [];
    const stmt = {
      bind(...a: unknown[]) {
        args = a;
        return stmt;
      },
      async run() {
        db.prepare(sql).run(...(args as never[]));
        return { success: true };
      },
      async first() {
        return db.prepare(sql).get(...(args as never[])) ?? null;
      },
      async all() {
        return { results: db.prepare(sql).all(...(args as never[])) };
      },
      async raw() {
        return db.prepare(sql).all(...(args as never[])).map((row) => Object.values(row as object));
      },
    };
    return stmt;
  };
  return { prepare } as unknown as D1Database;
}

/** A `Paddle-Signature` header for `body`, as Paddle would send it. */
export async function sign(body: string, secret: string, ts: number): Promise<string> {
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey("raw", enc.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, enc.encode(`${ts}:${body}`)));
  return `ts=${ts};h1=${[...mac].map((b) => b.toString(16).padStart(2, "0")).join("")}`;
}

/** A Paddle `subscription.*` event body. */
export function event(over: Record<string, unknown> = {}, data: Record<string, unknown> = {}): string {
  return JSON.stringify({
    event_type: "subscription.created",
    occurred_at: "2026-10-30T00:00:00.000Z",
    ...over,
    data: {
      id: "sub_01",
      customer_id: "ctm_01",
      status: "active",
      current_billing_period: { ends_at: "2026-11-30T00:00:00.000Z" },
      custom_data: { install_id: "install-1" },
      transaction_id: "txn_abcdefghij",
      ...data,
    },
  });
}
