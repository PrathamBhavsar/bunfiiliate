import { test } from "node:test";
import assert from "node:assert/strict";
import { BunfiiliateServer, BunfiiliateError, signEvent } from "../src/index.ts";

const ev = { type: "sale" as const, external_id: "o1", amount_cents: 100, currency: "USD" };
function mk(statuses: number[]) {
  const calls: { url: string; init: RequestInit }[] = [];
  const fetch = (async (url: string, init: RequestInit) => {
    calls.push({ url, init });
    const s = statuses[Math.min(calls.length - 1, statuses.length - 1)];
    return new Response(JSON.stringify(s < 300 ? { status: "accepted", id: "u1" } : { error: { code: "x", message: "m" } }), { status: s });
  }) as unknown as typeof globalThis.fetch;
  return { calls, c: new BunfiiliateServer({ baseUrl: "https://t.example/", programKey: "pub_1", secret: "s", fetch, backoffMs: 0 }) };
}

test("retries 503 twice then succeeds with the same Idempotency-Key", async () => {
  const { c, calls } = mk([503, 503, 202]);
  const r = await c.sendEvent(ev);
  assert.deepEqual(r, { status: "accepted", id: "u1" });
  assert.equal(calls.length, 3);
  const keys = new Set(calls.map((x) => (x.init.headers as Record<string, string>)["idempotency-key"]));
  assert.equal(keys.size, 1);
  assert.equal(calls[0].url, "https://t.example/v2/events/pub_1");
});

test("signature on the wire verifies for the sent key and body", async () => {
  const { c, calls } = mk([202]);
  await c.sendEvent(ev, { idempotencyKey: "k-1" });
  const h = calls[0].init.headers as Record<string, string>;
  const t = Number(/t=(\d+)/.exec(h["bf-signature"])![1]);
  assert.equal(h["bf-signature"], signEvent("s", "k-1", calls[0].init.body as string, t));
});

test("does not retry 400", async () => {
  const { c, calls } = mk([400]);
  await assert.rejects(c.sendEvent(ev), (e: unknown) => e instanceof BunfiiliateError && e.status === 400 && e.code === "x");
  assert.equal(calls.length, 1);
});

test("gives up after 3 retries on 429", async () => {
  const { c, calls } = mk([429]);
  await assert.rejects(c.sendEvent(ev), BunfiiliateError);
  assert.equal(calls.length, 4);
});

test("duplicate 200 is returned", async () => {
  const fetch = (async () => new Response('{"status":"duplicate","id":"u0"}', { status: 200 })) as unknown as typeof globalThis.fetch;
  const c = new BunfiiliateServer({ baseUrl: "https://t", programKey: "p", secret: "s", fetch });
  assert.deepEqual(await c.sendEvent(ev), { status: "duplicate", id: "u0" });
});

test("rejects float amount client-side", async () => {
  const { c, calls } = mk([202]);
  await assert.rejects(c.sendEvent({ ...ev, amount_cents: 49.5 }));
  assert.equal(calls.length, 0);
});
