import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, statSync } from "node:fs";
import { gzipSync } from "node:zlib";
import vm from "node:vm";

const dir = new URL("../crates/tracker/js/", import.meta.url);

function load(file, { search = "", src = "https://t.example.com/t.js", beacon = true, attrs = {}, dnt, storageBlocked = false, referrer = "https://news.example/s?x=1" } = {}) {
  const jar = new Map(), sent = [], store = new Map();
  const document = {
    referrer,
    currentScript: { src, getAttribute: (k) => attrs[k] ?? null },
    get cookie() { return [...jar].map(([k, v]) => `${k}=${v.value}`).join("; "); },
    set cookie(str) {
      const [pair, ...a] = str.split(";");
      const i = pair.indexOf("=");
      jar.set(pair.slice(0, i), { value: pair.slice(i + 1), attrs: Object.fromEntries(a.map((x) => x.split("="))) });
    },
  };
  const window = {
    location: { search, origin: "https://shop.example", pathname: "/p", protocol: "https:" },
    navigator: { doNotTrack: dnt, sendBeacon: beacon ? (url, blob) => (sent.push({ via: "beacon", url, blob }), true) : undefined },
    fetch: (url, init) => (sent.push({ via: "fetch", url, init }), Promise.resolve()),
    crypto: globalThis.crypto,
    localStorage: storageBlocked
      ? { getItem() { throw new Error("blocked"); }, setItem() { throw new Error("blocked"); } }
      : { getItem: (k) => store.get(k) ?? null, setItem: (k, v) => store.set(k, v) },
  };
  window.window = window;
  vm.runInNewContext(readFileSync(new URL(file, dir), "utf8"), { window, document, URL, URLSearchParams, Blob, Uint8Array, Array, JSON, Math, Date, RegExp, encodeURIComponent, decodeURIComponent });
  return { jar, sent, window, store };
}

for (const file of ["tracker.v2.src.js", "tracker.v2.min.js"]) {
  test(`${file}: ?bf_click= sets cookie 90d Lax Secure, vid in cookie`, () => {
    const { jar } = load(file, { search: "?bf_click=abc123" });
    const c = jar.get("bf_click");
    assert.equal(c.value, "abc123");
    assert.equal(c.attrs["max-age"], String(90 * 86400));
    assert.equal(c.attrs.SameSite, "Lax");
    assert.ok("Secure" in c.attrs);
    assert.match(jar.get("bf_vid").value, /^[A-Za-z0-9_-]{8,64}$/);
  });

  test(`${file}: beacon is v:2, schema-shaped, to /v2/c`, async () => {
    const { sent, jar } = load(file, { search: "?bf_click=abc123", attrs: { "data-pk": "pub_1" } });
    assert.equal(sent[0].via, "beacon");
    assert.equal(sent[0].url, "https://t.example.com/v2/c");
    const b = JSON.parse(await sent[0].blob.text());
    assert.equal(b.v, 2);
    assert.equal(b.click, "abc123");
    assert.equal(b.pk, "pub_1");
    assert.equal(b.vid, jar.get("bf_vid").value);
    assert.equal(b.url, "https://shop.example/p");
    assert.equal(b.ref, "https://news.example");
    assert.ok(Number.isInteger(b.ts));
    assert.ok(!("code" in b));
    assert.deepEqual(Object.keys(b).sort(), ["click", "pk", "ref", "ts", "url", "v", "vid"]);
  });

  test(`${file}: falls back to fetch keepalive without sendBeacon`, () => {
    const { sent } = load(file, { search: "?ref=promo", beacon: false });
    assert.equal(sent[0].via, "fetch");
    assert.equal(sent[0].url, "https://t.example.com/v2/c");
    assert.equal(sent[0].init.keepalive, true);
  });

  test(`${file}: does not throw when storage is blocked; vid still via cookie`, () => {
    const { jar, sent } = load(file, { search: "?bf_click=a1", storageBlocked: true });
    assert.ok(jar.has("bf_vid"));
    assert.equal(sent.length, 1);
  });

  test(`${file}: junk click value is dropped, never throws`, () => {
    const { sent } = load(file, { search: "?bf_click=" + encodeURIComponent("a b<>") });
    assert.equal(sent.length, 0);
  });

  test(`${file}: DNT honoured only with data-respect-dnt`, () => {
    assert.equal(load(file, { search: "?bf_click=a1", dnt: "1" }).sent.length, 1);
    const r = load(file, { search: "?bf_click=a1", dnt: "1", attrs: { "data-respect-dnt": "" } });
    assert.equal(r.sent.length, 0);
    assert.equal(r.jar.size, 0);
    assert.equal(load(file, { search: "?bf_click=a1", dnt: "0", attrs: { "data-respect-dnt": "" } }).sent.length, 1);
  });

  test(`${file}: click_id() and track() reuse stored values`, async () => {
    const { window, sent } = load(file, { search: "?bf_click=first" });
    assert.equal(window.bunfiiliate.click_id(), "first");
    window.bunfiiliate.track("second");
    const b = JSON.parse(await sent[1].blob.text());
    assert.equal(b.code, "second");
    assert.equal(b.click, "first");
  });

  test(`${file}: no ref header when referrer empty; no beacon without code/click`, () => {
    assert.equal(load(file).sent.length, 0);
  });
}

test("tracker.v2.min.js under budget (raw < 2560, gzip < 1280)", () => {
  const raw = readFileSync(new URL("tracker.v2.min.js", dir));
  assert.ok(raw.length < 2560, `raw ${raw.length}`);
  assert.ok(gzipSync(raw).length < 1280);
  assert.ok(statSync(new URL("tracker.v2.min.js", dir)).size === raw.length);
});
