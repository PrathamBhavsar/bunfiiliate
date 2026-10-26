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
    referrer: "https://news.example/",
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
      href: `${https ? "https" : "http"}://shop.example/p${search}`,
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
  test(`${file}: bf_click sets bf_vid (1y) and bf_ref (default 30d)`, () => {
    const { jar } = load(file, { search: "?bf_click=abc123" });
    assert.equal(jar.get("bf_ref").value, "abc123");
    assert.equal(jar.get("bf_ref").attrs["max-age"], String(30 * 86400));
    assert.equal(jar.get("bf_vid").attrs["max-age"], String(365 * 86400));
    assert.match(jar.get("bf_vid").value, /^[0-9a-f-]{32,36}$/);
    assert.equal(jar.get("bf_ref").attrs.path, "/");
    assert.ok("Secure" in jar.get("bf_ref").attrs);
  });

  test(`${file}: ?ref= wins over bf_click, data-days configures bf_ref`, () => {
    const { jar } = load(file, { search: "?ref=fromref&bf_click=other", days: "7" });
    assert.equal(jar.get("bf_ref").value, "fromref");
    assert.equal(jar.get("bf_ref").attrs["max-age"], String(7 * 86400));
  });

  test(`${file}: beacon posts {code,vid,url,ref} to script origin + /c`, async () => {
    const { jar, sent } = load(file, { search: "?bf_click=abc123" });
    assert.equal(sent.length, 1);
    assert.equal(sent[0].via, "beacon");
    assert.equal(sent[0].url, "https://t.example.com/c");
    const body = JSON.parse(await sent[0].blob.text());
    assert.deepEqual(body, {
      code: "abc123",
      vid: jar.get("bf_vid").value,
      url: "https://shop.example/p?bf_click=abc123",
      ref: "https://news.example/",
    });
  });

  test(`${file}: falls back to fetch keepalive without sendBeacon`, () => {
    const { sent } = load(file, { search: "?ref=x", beacon: false });
    assert.equal(sent.length, 1);
    assert.equal(sent[0].via, "fetch");
    assert.equal(sent[0].init.method, "POST");
    assert.equal(sent[0].init.keepalive, true);
  });

  test(`${file}: no code -> vid cookie only, no beacon, no bf_ref`, () => {
    const { jar, sent } = load(file);
    assert.ok(jar.has("bf_vid"));
    assert.ok(!jar.has("bf_ref"));
    assert.equal(sent.length, 0);
  });

  test(`${file}: track() reuses stored bf_ref and vid; explicit code overrides`, () => {
    const { jar, sent, window } = load(file, { search: "?bf_click=first" });
    const vid = jar.get("bf_vid").value;
    window.bunfiiliate.track();
    window.bunfiiliate.track("second");
    assert.equal(sent.length, 3);
    assert.equal(jar.get("bf_vid").value, vid);
    assert.equal(jar.get("bf_ref").value, "second");
  });

  test(`${file}: http page omits Secure`, () => {
    const { jar } = load(file, { search: "?ref=x", https: false });
    assert.ok(!("Secure" in jar.get("bf_ref").attrs));
  });
}
