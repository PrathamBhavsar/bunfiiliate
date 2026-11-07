// <x-docs>: markdown pages from /content/docs/*.md, hash routed (#/slug), sidebar + toc + search + prev/next.
import "./components.js"; // nav + footer
import brand from "../brand.js";
import { marked } from "marked";
import { icon } from "./icons.js";
import { esc, copyText } from "./util.js";

const raw = import.meta.glob("/content/docs/*.md", { query: "?raw", import: "default", eager: true });
const slugify = (s) => s.toLowerCase().replace(/<[^>]+>/g, "").replace(/[^\w\s-]/g, "").trim().replace(/\s+/g, "-");

const pages = Object.entries(raw).map(([path, src]) => {
  const m = src.match(/^---\n([\s\S]*?)\n---\n?([\s\S]*)$/);
  const meta = Object.fromEntries((m?.[1] ?? "").split("\n").map((l) => l.split(/:\s*(.*)/s).slice(0, 2)));
  const body = m ? m[2] : src;
  const heads = [...body.matchAll(/^(#{2,3}) (.+)$/gm)].map((h) => ({ depth: h[1].length, text: h[2].replace(/`/g, ""), id: slugify(h[2]) }));
  return { slug: path.split("/").pop().replace(/\.md$/, ""), title: meta.title, group: meta.group || "Docs", order: +meta.order || 99, desc: meta.description || "", body, heads };
}).sort((a, b) => a.order - b.order);

const renderer = new marked.Renderer();
renderer.heading = ({ text, depth }) => `<h${depth} id="${slugify(text)}">${text}</h${depth}>`;
renderer.code = ({ text }) => `<div class="cb"><pre><code>${esc(text)}</code></pre><button type="button" aria-label="Copy code" data-copy>${icon("copy")}</button></div>`;
marked.use({ renderer, gfm: true });
const html = (src) => marked.parse(src).replaceAll("<table>", '<div class="tbl"><table>').replaceAll("</table>", "</table></div>");

customElements.define("x-docs", class extends HTMLElement {
  connectedCallback() {
    const groups = [...new Set(pages.map((p) => p.group))];
    this.innerHTML = `<div class="wrap docs">
      <button class="docs-menu btn" aria-expanded="false">${icon("menu")} Pages</button>
      <aside class="side" aria-label="Documentation">
        <button class="search-open" type="button">${icon("search")}<span>Search docs</span><kbd>/</kbd></button>
        ${groups.map((g) => `<h2>${esc(g)}</h2><ul>${pages.filter((p) => p.group === g).map((p) => `<li><a href="#/${p.slug}" data-slug="${p.slug}">${esc(p.title)}</a></li>`).join("")}</ul>`).join("")}
      </aside>
      <article class="doc" tabindex="-1"></article>
      <nav class="toc" aria-label="On this page"></nav>
    </div>
    <dialog class="search"><form method="dialog"><input type="search" placeholder="Search ${esc(brand.name)} docs" aria-label="Search docs" autocomplete="off" /><ul></ul></form></dialog>`;
    this.article = this.querySelector(".doc");
    this.querySelector(".docs-menu").onclick = (e) => { const o = this.querySelector(".side").toggleAttribute("data-open"); e.currentTarget.setAttribute("aria-expanded", o); };
    this.article.addEventListener("click", async (e) => {
      const b = e.target.closest("[data-copy]"); if (!b) return;
      if (await copyText(b.parentElement.querySelector("code").textContent)) { b.innerHTML = icon("check"); setTimeout(() => (b.innerHTML = icon("copy")), 1500); }
    });
    this.querySelector(".toc").addEventListener("click", (e) => {
      const a = e.target.closest("a"); if (!a) return; e.preventDefault();
      document.getElementById(a.dataset.id)?.scrollIntoView({ behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
    });
    this.initSearch();
    addEventListener("hashchange", () => this.route());
    this.route();
  }

  route() {
    const slug = location.hash.replace(/^#\//, "") || pages[0].slug;
    const i = Math.max(0, pages.findIndex((p) => p.slug === slug));
    const p = pages[i];
    document.title = `${p.title} · ${brand.name} docs`;
    const crumb = `<p class="crumb">${esc(p.group)}</p>`;
    const prev = pages[i - 1], next = pages[i + 1];
    this.article.innerHTML = `${crumb}${html(p.body)}<div class="pn">${prev ? `<a href="#/${prev.slug}"><span>Previous</span>${esc(prev.title)}</a>` : "<span></span>"}${next ? `<a class="next" href="#/${next.slug}"><span>Next</span>${esc(next.title)}</a>` : ""}</div>`;
    this.querySelectorAll(".side a").forEach((a) => (a.toggleAttribute("aria-current", a.dataset.slug === p.slug), a.dataset.slug === p.slug && a.setAttribute("aria-current", "page")));
    this.querySelector(".toc").innerHTML = p.heads.length ? `<h2>On this page</h2><ul>${p.heads.map((h) => `<li class="d${h.depth}"><a href="#${h.id}" data-id="${h.id}">${esc(h.text)}</a></li>`).join("")}</ul>` : "";
    this.querySelector(".side").removeAttribute("data-open");
    this.querySelector(".docs-menu").setAttribute("aria-expanded", "false");
    scrollTo(0, 0);
  }

  initSearch() {
    const dlg = this.querySelector("dialog"), input = dlg.querySelector("input"), list = dlg.querySelector("ul");
    let sel = 0, hits = [];
    const render = () => {
      const q = input.value.trim().toLowerCase();
      hits = (q ? pages.map((p) => {
        const body = p.body.toLowerCase(), at = body.indexOf(q);
        const score = (p.title.toLowerCase().includes(q) ? 3 : 0) + (p.heads.some((h) => h.text.toLowerCase().includes(q)) ? 2 : 0) + (at >= 0 ? 1 : 0);
        return { p, score, snip: at >= 0 ? p.body.slice(Math.max(0, at - 40), at + 80).replace(/[#`*\n]+/g, " ") : p.desc };
      }).filter((h) => h.score).sort((a, b) => b.score - a.score) : pages.map((p) => ({ p, snip: p.desc })));
      sel = 0;
      list.innerHTML = hits.length ? hits.map((h, i) => `<li><a href="#/${h.p.slug}" ${i === 0 ? "aria-selected='true'" : ""}><b>${esc(h.p.title)}</b><span>${esc(h.snip)}</span></a></li>`).join("") : `<li class="none">No pages match “${esc(q)}”.</li>`;
    };
    const open = () => { input.value = ""; render(); dlg.showModal(); input.focus(); };
    this.querySelector(".search-open").onclick = open;
    addEventListener("keydown", (e) => {
      if ((e.key === "k" && (e.metaKey || e.ctrlKey)) || (e.key === "/" && !/input|textarea/i.test(document.activeElement?.tagName))) { e.preventDefault(); open(); }
    });
    input.oninput = render;
    input.onkeydown = (e) => {
      const links = [...list.querySelectorAll("a")];
      if (e.key === "ArrowDown" || e.key === "ArrowUp") { e.preventDefault(); sel = (sel + (e.key === "ArrowDown" ? 1 : -1) + links.length) % links.length; links.forEach((a, i) => a.toggleAttribute("aria-selected", i === sel)); links[sel]?.scrollIntoView({ block: "nearest" }); }
      if (e.key === "Enter") { e.preventDefault(); links[sel]?.click(); }
    };
    list.onclick = () => dlg.close();
    dlg.addEventListener("click", (e) => { if (e.target === dlg) dlg.close(); });
  }
});
