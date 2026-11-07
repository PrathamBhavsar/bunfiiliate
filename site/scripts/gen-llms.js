// Emits public/llms.txt: an index of doc pages for agents (same idea as Mintlify's /llms.txt).
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const dir = join(import.meta.dirname, "../content/docs");
const pages = readdirSync(dir).filter((f) => f.endsWith(".md")).map((f) => {
  const src = readFileSync(join(dir, f), "utf8");
  const meta = Object.fromEntries((src.match(/^---\n([\s\S]*?)\n---/)?.[1] ?? "").split("\n").map((l) => l.split(/:\s*(.*)/s).slice(0, 2)));
  return { slug: f.replace(/\.md$/, ""), title: meta.title, order: +meta.order || 99, desc: meta.description || "" };
}).sort((a, b) => a.order - b.order);

const { default: brand } = await import("../src/brand.js");
const site = process.env.SITE_URL || `https://${brand.repo.split("/")[0].toLowerCase()}.github.io/${brand.repo.split("/")[1]}`;
const out = `# ${brand.name}\n\n> ${brand.footBlurb}\n\n## Docs\n\n${pages.map((p) => `- [${p.title}](${site}/docs/#/${p.slug}): ${p.desc}`).join("\n")}\n\n## Source\n\n- [GitHub](https://github.com/${brand.repo})\n`;
writeFileSync(join(import.meta.dirname, "../public/llms.txt"), out);
console.log(`llms.txt: ${pages.length} pages`);
