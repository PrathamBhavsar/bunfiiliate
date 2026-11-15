// Browser smoke test: node scripts/smoke.mjs <base-url>. Needs playwright-core + a chromium.
import { chromium } from "playwright-core";
import assert from "node:assert/strict";

const base = (process.argv[2] || "http://localhost:4173").replace(/\/$/, "");
const b = await chromium.launch({ executablePath: process.env.CHROME_PATH || undefined });
const ctx = await b.newContext({ viewport: { width: 1280, height: 800 }, permissions: ["clipboard-read", "clipboard-write"] });
const p = await ctx.newPage();
const errors = [];
p.on("pageerror", (e) => errors.push(e.message));
p.on("console", (m) => m.type() === "error" && !/api\.github\.com|Failed to load resource/.test(m.text()) && errors.push(m.text()));

await p.goto(base + "/", { waitUntil: "networkidle" });
assert.ok(await p.locator("h1").isVisible(), "hero h1 visible");
const shot = p.locator(".hero .shot img");
assert.ok((await shot.count()) === 1, "hero product screenshot mounted");
assert.ok(await shot.evaluate((i) => i.complete && i.naturalWidth > 0), "hero screenshot loaded");
assert.ok(((await shot.getAttribute("alt")) || "").length > 20, "hero screenshot has alt text");
await p.locator("x-hero x-copy button").click();
assert.match(await p.evaluate(() => navigator.clipboard.readText()), /^curl|^cargo|^npx|^make|^git/, "copy button copies install command");
const tabs = p.locator("x-install [role=tab]");
assert.ok((await tabs.count()) >= 2, "install tabs");
const before = await p.locator("x-install pre").innerText();
await tabs.nth(1).click();
assert.notEqual(await p.locator("x-install pre").innerText(), before, "tab switches content");
assert.ok((await p.locator("x-features .group").count()) >= 3, "feature groups");

await p.goto(base + "/docs/", { waitUntil: "networkidle" });
const links = await p.locator(".side li a").count();
assert.ok(links >= 5, "docs sidebar");
for (let i = 0; i < links; i++) {
  await p.locator(".side li a").nth(i).click();
  assert.ok(await p.locator(".doc h1").isVisible(), `doc page ${i} has h1`);
}
await p.keyboard.press("/");
assert.ok(await p.locator("dialog.search").evaluate((d) => d.open), "search opens on /");
await p.keyboard.type("install");
assert.ok((await p.locator("dialog.search li a").count()) >= 1, "search returns hits");

assert.deepEqual(errors, [], "no console errors");
await b.close();
console.log("smoke ok");
