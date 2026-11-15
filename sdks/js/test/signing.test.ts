import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createHmac } from "node:crypto";
import { signEvent, verifyOutbound } from "../src/index.ts";

const vec = JSON.parse(readFileSync(new URL("../../../schemas/tracker/v2/examples/signature-vector.json", import.meta.url), "utf8"));

test("signEvent matches the shared vector", () => {
  assert.equal(signEvent(vec.secret, vec.idempotency_key, vec.body, vec.t), vec.header);
});

test("signEvent rejects a bad idempotency key", () => {
  assert.throws(() => signEvent("s", "bad key", "{}", 1));
  assert.throws(() => signEvent("s", "", "{}", 1));
});

const out = (secret: string, t: number, body: string) => `t=${t},v1=${createHmac("sha256", secret).update(`${t}.${body}`).digest("hex")}`;
const now = 1_800_000_000;

test("verifyOutbound accepts valid", () => {
  const body = '{"id":"evt_1"}';
  assert.ok(verifyOutbound("sec", { "Bf-Signature": out("sec", now, body) }, body, 300, now));
  assert.ok(verifyOutbound("sec", new Headers({ "bf-signature": out("sec", now, body) }), body, 300, now));
});

test("verifyOutbound rejects tamper, wrong secret, missing header", () => {
  const h = { "bf-signature": out("sec", now, "a") };
  assert.ok(!verifyOutbound("sec", h, "b", 300, now));
  assert.ok(!verifyOutbound("other", h, "a", 300, now));
  assert.ok(!verifyOutbound("sec", {}, "a", 300, now));
  assert.ok(!verifyOutbound("sec", { "bf-signature": "t=1,v1=zz" }, "a", 300, now));
});

test("verifyOutbound rejects old and future timestamps", () => {
  assert.ok(!verifyOutbound("sec", { "bf-signature": out("sec", now - 301, "a") }, "a", 300, now));
  assert.ok(!verifyOutbound("sec", { "bf-signature": out("sec", now + 301, "a") }, "a", 300, now));
  assert.ok(verifyOutbound("sec", { "bf-signature": out("sec", now - 300, "a") }, "a", 300, now));
});

test("verifyOutbound accepts any of multiple v1", () => {
  const good = out("new", now, "a").split(",")[1];
  const h = { "bf-signature": `t=${now},v1=${"00".repeat(32)},${good}` };
  assert.ok(verifyOutbound("new", h, "a", 300, now));
});
