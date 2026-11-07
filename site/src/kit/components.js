// Landing components. Light DOM custom elements (shared CSS), content from ../brand.js.
import brand from "../brand.js";
import { icon } from "./icons.js";
import { base, esc, md, copyText, ghFacts } from "./util.js";

const define = (name, cls) => customElements.define(name, cls);
const facts = ghFacts(brand.repo); // one fetch shared by every component

const mark = () => `<a class="mark" href="${base}" aria-label="${esc(brand.name)} home">${brand.wordmark}</a>`;

define("x-nav", class extends HTMLElement {
  connectedCallback() {
    const here = location.pathname.includes("/docs") ? "docs" : "home";
    const links = [["Features", `${base}#features`], ["How it works", `${base}#how`], ["Install", `${base}#install`], ["Docs", `${base}docs/`]];
    this.innerHTML = `<div class="wrap nav">
      ${mark()}
      <nav class="nav-links" aria-label="Primary">${links.map(([t, h]) => `<a href="${h}"${t === "Docs" && here === "docs" ? " aria-current='page'" : ""}>${t}</a>`).join("")}</nav>
      <div class="nav-end">
        <a class="btn" href="https://github.com/${brand.repo}">${icon("github")}<span>GitHub</span><span class="stars" data-stars></span></a>
        <a class="btn primary" href="${base}docs/#/quickstart">Get started</a>
      </div>
      <button class="nav-toggle" aria-expanded="false" aria-label="Menu">${icon("menu")}</button>
    </div>`;
    const nav = this.querySelector(".nav"), tg = this.querySelector(".nav-toggle");
    tg.onclick = () => { const o = nav.toggleAttribute("data-open"); tg.setAttribute("aria-expanded", o); };
    nav.addEventListener("click", (e) => { if (e.target.closest(".nav-links a")) { nav.removeAttribute("data-open"); tg.setAttribute("aria-expanded", false); } });
    facts.then((f) => { if (f) this.querySelector("[data-stars]").textContent = "★ " + f.stars; });
  }
});

define("x-copy", class extends HTMLElement {
  connectedCallback() {
    const cmd = this.getAttribute("cmd");
    this.innerHTML = `<code>${esc(cmd)}</code><button type="button" aria-label="Copy install command">${icon("copy")}</button>`;
    const b = this.querySelector("button");
    b.onclick = async () => {
      if (!(await copyText(cmd))) return;
      this.toggleAttribute("data-done", true); b.innerHTML = icon("check");
      setTimeout(() => { this.removeAttribute("data-done"); b.innerHTML = icon("copy"); }, 1600);
    };
  }
});

define("x-hero", class extends HTMLElement {
  connectedCallback() {
    this.innerHTML = `<section class="hero">
      <x-ascii scene="${brand.scene}"></x-ascii>
      <div class="wrap hero-inner">
        <h1 class="display">${brand.headline}</h1>
        <p class="lede">${brand.sub}</p>
        <x-copy cmd="${esc(brand.heroCmd)}"></x-copy>
        <div class="cta">
          <a class="btn primary" href="${base}docs/#/quickstart">Get started ${icon("arrow")}</a>
          <a class="btn" href="https://github.com/${brand.repo}">${icon("github")} Star on GitHub</a>
        </div>
        <p class="hero-meta"><span data-tag>${esc(brand.status)}</span><span>${esc(brand.license)}</span><span>${esc(brand.stack)}</span></p>
      </div>
    </section>`;
    facts.then((f) => { if (f?.tag) this.querySelector("[data-tag]").textContent = f.tag + (f.pre ? " (pre-release)" : ""); });
  }
});

define("x-live", class extends HTMLElement {
  connectedCallback() {
    const row = (k, v) => `<li><span class="k">${esc(k)}</span><span class="dots"></span><span class="v">${v}</span></li>`;
    const draw = (f) => {
      const live = f ? [
        row("GitHub stars", f.stars),
        row("Releases", f.releases ? `<a href="https://github.com/${brand.repo}/releases">${f.releases}${f.tag ? " · " + esc(f.tag) : ""}</a>` : "none yet (pre-release)"),
        row("Release downloads", f.downloads),
      ] : [row("Source", `<a href="https://github.com/${brand.repo}">github.com/${esc(brand.repo)}</a>`)];
      this.querySelector(".receipt").innerHTML = live.concat(brand.facts.map(([k, v]) => row(k, md(v)))).join("");
    };
    this.innerHTML = `<div class="wrap live">
      <div><h2 class="display">${brand.liveTitle}</h2><p class="note">${md(brand.liveNote)}</p></div>
      <ul class="receipt" aria-label="Project facts"></ul>
    </div>`;
    draw(null); facts.then(draw);
  }
});

define("x-features", class extends HTMLElement {
  connectedCallback() {
    this.id = "features";
    this.innerHTML = `<div class="wrap">
      <h2 class="display sec-head">${brand.featuresTitle}</h2>
      <p class="sec-lede">${md(brand.featuresLede)}</p>
      <div class="groups">${brand.features.map((g) => `<section class="group">
        <div class="group-head"><h3 class="display">${esc(g.title)}</h3><p>${md(g.blurb)}</p></div>
        <ul class="rows">${g.items.map(([ic, t, d]) => `<li>${icon(ic)}<div><h4>${esc(t)}</h4><p>${md(d)}</p></div></li>`).join("")}</ul>
      </section>`).join("")}</div>
    </div>`;
  }
});

define("x-steps", class extends HTMLElement {
  connectedCallback() {
    this.id = "how";
    this.innerHTML = `<div class="wrap">
      <h2 class="display sec-head">${brand.howTitle}</h2>
      <p class="sec-lede">${md(brand.howLede)}</p>
      <ol class="flow">${brand.steps.map(([t, d]) => `<li><span class="node"></span><h3>${esc(t)}</h3><p>${md(d)}</p></li>`).join("")}</ol>
    </div>`;
  }
});

define("x-install", class extends HTMLElement {
  connectedCallback() {
    this.id = "install";
    const tabs = brand.install;
    this.innerHTML = `<div class="wrap install-grid">
      <div>
        <h2 class="display sec-head">${brand.installTitle}</h2>
        <p class="sec-lede">${md(brand.installLede)}</p>
        <div class="cta"><a class="btn ink" href="${base}docs/#/install">Full install guide ${icon("arrow")}</a><a class="btn ink" href="https://github.com/${brand.repo}/releases">${icon("download")} Releases</a></div>
      </div>
      <div>
        <div role="tablist" aria-label="Install method">${tabs.map((t, i) => `<button role="tab" id="tab-${t.id}" aria-controls="panel" aria-selected="${i === 0}" tabindex="${i ? -1 : 0}">${esc(t.label)}</button>`).join("")}</div>
        <div class="code" id="panel" role="tabpanel"><pre></pre><button type="button" aria-label="Copy commands">${icon("copy")}</button></div>
        <p class="tab-note"></p>
      </div>
    </div>`;
    const pre = this.querySelector("pre"), note = this.querySelector(".tab-note"), box = this.querySelector(".code");
    const btns = [...this.querySelectorAll("[role=tab]")];
    let cur = tabs[0];
    const show = (i, focus) => {
      cur = tabs[i];
      btns.forEach((b, j) => { b.setAttribute("aria-selected", j === i); b.tabIndex = j === i ? 0 : -1; });
      pre.innerHTML = cur.cmds.map((c) => c.startsWith("#") ? `<span class="c">${esc(c)}</span>` : `<span class="p">$ </span>${esc(c)}`).join("\n");
      pre.parentElement.setAttribute("aria-labelledby", "tab-" + cur.id);
      note.innerHTML = md(cur.note || "");
      if (focus) btns[i].focus();
    };
    btns.forEach((b, i) => { b.onclick = () => show(i); });
    this.querySelector("[role=tablist]").onkeydown = (e) => {
      const i = btns.findIndex((b) => b.getAttribute("aria-selected") === "true");
      const n = { ArrowRight: (i + 1) % btns.length, ArrowLeft: (i + btns.length - 1) % btns.length, Home: 0, End: btns.length - 1 }[e.key];
      if (n !== undefined) { e.preventDefault(); show(n, true); }
    };
    const cb = box.querySelector("button");
    cb.onclick = async () => {
      if (!(await copyText(cur.cmds.filter((c) => !c.startsWith("#")).join("\n")))) return;
      box.toggleAttribute("data-done", true); cb.innerHTML = icon("check");
      setTimeout(() => { box.removeAttribute("data-done"); cb.innerHTML = icon("copy"); }, 1600);
    };
    show(0);
  }
});

define("x-faq", class extends HTMLElement {
  connectedCallback() {
    this.innerHTML = `<div class="wrap">
      <h2 class="display sec-head">Questions people ask.</h2>
      <div class="faq">${brand.faq.map(([q, a]) => `<details><summary>${esc(q)}</summary><p>${md(a)}</p></details>`).join("")}</div>
    </div>`;
  }
});

define("x-footer", class extends HTMLElement {
  connectedCallback() {
    const col = (h, items) => `<div><h5>${h}</h5><ul>${items.map(([t, u]) => `<li><a href="${u}">${t}</a></li>`).join("")}</ul></div>`;
    const gh = `https://github.com/${brand.repo}`;
    this.innerHTML = `<div class="wrap">
      <div class="foot">
        <div>${mark()}<p>${esc(brand.footBlurb)}</p></div>
        ${col("Product", [["Features", `${base}#features`], ["Install", `${base}#install`], ["Releases", `${gh}/releases`]])}
        ${col("Docs", [["Quickstart", `${base}docs/#/quickstart`], ["Configuration", `${base}docs/#/configuration`], ["Security", `${base}docs/#/security`], ["llms.txt", `${base}llms.txt`]])}
        ${col("Project", [["GitHub", gh], ["Issues", `${gh}/issues`], ["Security policy", `${gh}/blob/staging/SECURITY.md`]])}
      </div>
      <div class="foot-end"><span>${esc(brand.license)}</span><span>${esc(brand.footNote)}</span></div>
    </div>`;
  }
});
