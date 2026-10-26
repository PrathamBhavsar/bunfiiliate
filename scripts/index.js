import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const dir = new URL("../crates/tracker/js/", import.meta.url);

// Run a tracker build against a fake browser; returns the cookie jar and sent beacons.
function load(file, { search = "", src = "https://t.example.com/t.js", days, beacon = true, https = true } = {}) {
  const jar = new Map();
  const sent = [];
  const document = {
    referrer: "https://news.example/story?id=1#x",
    currentScript: {
      src,
      getAttribute: (k) => (k === "data-days" ? days ?? null : null),
    },
    get cookie() {
      return [...jar].map(([k, v]) => `${k}=${v.value}`).join("; ");
    },
    set cookie(str) {
      const [pair, ...attrs] = str.split(";");
      const i = pair.indexOf("=");
      const entry = { value: pair.slice(i + 1), attrs: Object.fromEntries(attrs.map((a) => a.split("="))) };
      jar.set(pair.slice(0, i), entry);
    },
  };
  const window = {
    location: {
      search,
      origin: `${https ? "https" : "http"}://shop.example`,
      pathname: "/p",
      protocol: https ? "https:" : "http:",
    },
    navigator: {
      sendBeacon: beacon ? (url, blob) => (sent.push({ via: "beacon", url, blob }), true) : undefined,
    },
    fetch: (url, init) => (sent.push({ via: "fetch", url, init }), Promise.resolve()),
    crypto: globalThis.crypto,
  };
  window.window = window;
  vm.runInNewContext(readFileSync(new URL(file, dir), "utf8"), { window, document, URL, URLSearchParams, Blob, Uint8Array, Array, JSON, Math, encodeURIComponent, decodeURIComponent });
  return { jar, sent, window };
}

for (const file of ["tracker.src.js", "tracker.min.js"]) {
  test(`${file}: bf_click sets bf_vid (1y) and bf_click (default 30d), not bf_ref`, () => {
    const { jar } = load(file, { search: "?bf_click=abc123" });
    assert.equal(jar.get("bf_click").value, "abc123");
    assert.ok(!jar.has("bf_ref"));
    assert.equal(jar.get("bf_click").attrs["max-age"], String(30 * 86400));
    assert.equal(jar.get("bf_vid").attrs["max-age"], String(365 * 86400));
    assert.match(jar.get("bf_vid").value, /^[0-9a-f-]{32,36}$/);
    assert.equal(jar.get("bf_click").attrs.path, "/");
    assert.ok("Secure" in jar.get("bf_click").attrs);
  });

  test(`${file}: ?ref= is the code, bf_click the click; data-days configures both cookies`, async () => {
    const { jar, sent } = load(file, { search: "?ref=fromref&bf_click=other", days: "7" });
    assert.equal(jar.get("bf_ref").value, "fromref");
    assert.equal(jar.get("bf_click").value, "other");
    assert.equal(jar.get("bf_ref").attrs["max-age"], String(7 * 86400));
    assert.equal(jar.get("bf_click").attrs["max-age"], String(7 * 86400));
    const body = JSON.parse(await sent[0].blob.text());
    assert.equal(body.code, "fromref");
    assert.equal(body.click, "other");
  });

  test(`${file}: beacon posts {code,click,vid,url,ref} (query/hash stripped) to script origin + /c`, async () => {
    const { jar, sent } = load(file, { search: "?bf_click=abc123" });
    assert.equal(sent.length, 1);
    assert.equal(sent[0].via, "beacon");
    assert.equal(sent[0].url, "https://t.example.com/c");
    const body = JSON.parse(await sent[0].blob.text());
    assert.deepEqual(body, {
      code: null,
      click: "abc123",
      vid: jar.get("bf_vid").value,
      url: "https://shop.example/p",
      ref: "https://news.example",
    });
  });

  test(`${file}: falls back to fetch keepalive without sendBeacon`, () => {
    const { sent } = load(file, { search: "?ref=x", beacon: false });
    assert.equal(sent.length, 1);
    assert.equal(sent[0].via, "fetch");
    assert.equal(sent[0].init.method, "POST");
    assert.equal(sent[0].init.keepalive, true);
    assert.equal(sent[0].init.credentials, "omit");
    assert.equal(sent[0].init.headers["content-type"], "text/plain");
  });

  test(`${file}: the beacon is a text/plain Blob (a simple request, no CORS preflight)`, () => {
    const { sent } = load(file, { search: "?ref=x" });
    assert.equal(sent[0].via, "beacon");
    assert.equal(sent[0].blob.type, "text/plain");
  });

  test(`${file}: a click-only URL does not pair the new click with a stored code`, async () => {
    // load() has no way to pre-seed cookies, so seed via a first visit, then re-run on the same jar.
    const first = load(file, { search: "?ref=stale" });
    const again = load(file, { search: "?bf_click=newclick" });
    const body = JSON.parse(await again.sent[0].blob.text());
    assert.equal(body.code, null);
    assert.equal(body.click, "newclick");
    // An explicit track() afterwards still falls back to the stored values.
    first.window.bunfiiliate.track();
    assert.equal(JSON.parse(await first.sent[1].blob.text()).code, "stale");
  });

  test(`${file}: ?ref= alone posts the code with a null click`, async () => {
    const { jar, sent } = load(file, { search: "?ref=promo" });
    const body = JSON.parse(await sent[0].blob.text());
    assert.deepEqual(body, {
      code: "promo",
      click: null,
      vid: jar.get("bf_vid").value,
      url: "https://shop.example/p",
      ref: "https://news.example",
    });
    assert.ok(!jar.has("bf_click"));
  });

  test(`${file}: no code or click -> vid cookie only, no beacon, no bf_ref/bf_click`, () => {
    const { jar, sent } = load(file);
    assert.ok(jar.has("bf_vid"));
    assert.ok(!jar.has("bf_ref"));
    assert.ok(!jar.has("bf_click"));
    assert.equal(sent.length, 0);
  });

  test(`${file}: track() reuses stored click and vid; explicit code overrides`, async () => {
    const { jar, sent, window } = load(file, { search: "?bf_click=first" });
    const vid = jar.get("bf_vid").value;
    window.bunfiiliate.track();
    window.bunfiiliate.track("second");
    assert.equal(sent.length, 3);
    assert.equal(jar.get("bf_vid").value, vid);
    assert.equal(jar.get("bf_ref").value, "second");
    assert.equal(jar.get("bf_click").value, "first");
    const last = JSON.parse(await sent[2].blob.text());
    assert.equal(last.code, "second");
    assert.equal(last.click, "first");
  });

  test(`${file}: http page omits Secure`, () => {
    const { jar } = load(file, { search: "?ref=x", https: false });
    assert.ok(!("Secure" in jar.get("bf_ref").attrs));
  });
}
