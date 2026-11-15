// <x-ascii scene="name">: canvas glyph grid. A scene is registerScene(name, ({cols, rows, rand}) => ({ step(t, dt, ptr, put) })).
// put(col, row, ch, tone, alpha): tone 0 = dim, 1 = fg, 2 = accent.
const scenes = {};
export const registerScene = (name, make) => { scenes[name] = make; };

function rng(seed) { // mulberry32, so the reduced-motion still frame is stable
  return () => { seed |= 0; seed = (seed + 0x6d2b79f5) | 0; let t = Math.imul(seed ^ (seed >>> 15), 1 | seed); t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t; return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
}

class XAscii extends HTMLElement {
  connectedCallback() {
    this.setAttribute("aria-hidden", "true");
    this.canvas = document.createElement("canvas");
    this.append(this.canvas);
    this.ctx = this.canvas.getContext("2d");
    this.ptr = { col: -99, row: -99 };
    this.still = matchMedia("(prefers-reduced-motion: reduce)").matches;
    this.visible = true;
    this.ro = new ResizeObserver(() => this.layout());
    this.ro.observe(this);
    this.io = new IntersectionObserver(([e]) => { this.visible = e.isIntersecting; if (this.visible) this.kick(); });
    this.io.observe(this);
    const host = this.parentElement;
    host.addEventListener("pointermove", (e) => {
      const r = this.getBoundingClientRect();
      this.ptr = { col: Math.floor((e.clientX - r.left) / this.cw), row: Math.floor((e.clientY - r.top) / this.ch) };
    });
    host.addEventListener("pointerleave", () => { this.ptr = { col: -99, row: -99 }; });
    Promise.all([document.fonts.load('16px "Geist Pixel Square"'), document.fonts.load('16px "Geist Mono"')]).finally(() => { this.ready = true; this.layout(); });
  }
  disconnectedCallback() { this.ro?.disconnect(); this.io?.disconnect(); cancelAnimationFrame(this.raf); }

  layout() {
    if (!this.ready) return;
    const { width, height } = this.getBoundingClientRect();
    if (!width || !height) return;
    const dpr = Math.min(devicePixelRatio || 1, 2);
    const fs = width < 640 ? 13 : 15;
    this.canvas.width = width * dpr; this.canvas.height = height * dpr;
    const c = this.ctx;
    c.setTransform(dpr, 0, 0, dpr, 0, 0);
    this.pixFont = `${fs}px "Geist Pixel Square", monospace`;
    this.monoFont = `${fs - 1}px "Geist Mono", monospace`;
    c.font = this.pixFont;
    c.textBaseline = "top";
    this.cw = Math.ceil(c.measureText("0").width); this.ch = Math.round(fs * 1.3);
    this.cols = Math.floor(width / this.cw); this.rows = Math.floor(height / this.ch);
    const css = getComputedStyle(this);
    this.tones = [css.getPropertyValue("--fg-dim").trim(), css.getPropertyValue("--fg").trim(), css.getPropertyValue("--accent").trim()];
    this.scene = scenes[this.getAttribute("scene")]({ cols: this.cols, rows: this.rows, rand: rng(7) });
    this.t = 0;
    if (this.still) { for (let i = 0; i < 240; i++) this.scene.step(this.t += 33, 33, this.ptr, () => {}); this.draw(0); }
    else this.kick();
  }
  kick() { if (!this.running && !this.still && this.scene) { this.running = true; this.last = performance.now(); this.raf = requestAnimationFrame((n) => this.loop(n)); } }
  loop(now) {
    if (!this.visible || document.hidden) { this.running = false; return; }
    const dt = Math.min(now - this.last, 100);
    if (dt >= 32) { this.last = now; this.draw(dt); }
    this.raf = requestAnimationFrame((n) => this.loop(n));
  }
  draw(dt) {
    const c = this.ctx, tones = this.tones;
    c.clearRect(0, 0, this.canvas.width, this.canvas.height);
    let monoOn = false;
    this.scene.step((this.t += dt), dt, this.ptr, (col, row, ch, tone = 1, a = 1, mono = false) => {
      if (col < 0 || row < 0 || col >= this.cols || row >= this.rows || ch === " ") return;
      if (mono !== monoOn) { c.font = mono ? this.monoFont : this.pixFont; monoOn = mono; }
      c.globalAlpha = a;
      c.fillStyle = tones[tone];
      c.fillText(ch, col * this.cw, row * this.ch);
    });
    c.globalAlpha = 1;
  }
}
customElements.define("x-ascii", XAscii);
