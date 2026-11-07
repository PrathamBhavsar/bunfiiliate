// Click rain: drops fall onto a ledger. Each landing posts a balanced two-line transaction, and the sheet behind it
// scrolls paired +/- figures that cancel. SUM stays 0.00.
import { registerScene } from "../kit/ascii.js";

const fmt = (cents) => (cents < 0 ? "-" : "+") + (Math.abs(cents) / 100).toFixed(2);
const hash = (n) => { n = Math.imul(n ^ (n >>> 16), 0x45d9f3b); n = Math.imul(n ^ (n >>> 16), 0x45d9f3b); return (n ^ (n >>> 16)) >>> 0; };

registerScene("rain", ({ cols, rows, rand }) => {
  const wide = cols >= 72;
  const lw = wide ? Math.min(40, Math.floor(cols * 0.34)) : Math.min(34, cols - 2);
  const line = (l, r) => l + " ".repeat(Math.max(1, lw - l.length - r.length)) + r;
  const x0 = wide ? cols - lw - 3 : Math.floor((cols - lw) / 2);
  const ledgerH = 10;
  const top = rows - ledgerH - 1;
  const drops = [], splashes = [], txns = [];
  let acc = 0, id = 4800 + Math.floor(rand() * 90);

  const spawn = () => drops.push({ x: Math.floor(rand() * cols), y: -2, v: 0.006 + rand() * 0.009, len: 5 + Math.floor(rand() * 7) });
  for (let i = 0; i < cols * 0.6; i++) { spawn(); drops[i].y = rand() * rows; }

  const post = () => {
    txns.unshift({ id: ++id, cents: 100 + Math.floor(rand() * 9900) });
    if (txns.length > 3) txns.pop();
  };

  return {
    step(t, dt, ptr, put) {
      const text = (r, s, tone, a) => { for (let i = 0; i < s.length; i++) put(x0 + i, r, s[i], tone, a, true); };

      // the sheet: rows k and k+1 share an amount with opposite signs, so any pair sums to zero
      if (wide) {
        const k0 = Math.floor(t / 650);
        for (let r = 0; r < top - 1; r++) {
          const k = k0 + r, amt = 100 + (hash(k >> 1) % 9900);
          text(r, fmt(k & 1 ? -amt : amt).padStart(lw), 0, 0.16 + 0.1 * (r / top));
        }
      }

      acc += dt * cols * 0.00014;
      while (acc >= 1) { spawn(); acc -= 1; }
      for (let i = drops.length - 1; i >= 0; i--) {
        const d = drops[i];
        d.y += d.v * dt;
        const dx = d.x - ptr.col, dy = d.y - ptr.row;
        if (Math.abs(dx) < 6 && Math.abs(dy) < 8) d.x += Math.sign(dx || 1) * 0.12;
        const c = Math.round(d.x);
        const inBand = c >= x0 && c < x0 + lw;
        const floor = inBand ? top : rows - 1;
        if (d.y >= floor) {
          if (inBand) { post(); splashes.push({ c, r: top, life: 1, hot: true }); }
          else splashes.push({ c, r: rows - 1, life: 0.7, hot: false });
          drops.splice(i, 1);
          continue;
        }
        const r = Math.floor(d.y);
        put(c, r, "*", 2, 1);
        for (let k = 1; k < d.len; k++) put(c, r - k, k < 3 ? "+" : k < 5 ? ":" : ".", k < 3 ? 1 : 0, 0.95 - k / (d.len + 1));
      }
      for (let i = splashes.length - 1; i >= 0; i--) {
        const s = splashes[i]; s.life -= dt * 0.0016;
        if (s.life <= 0) { splashes.splice(i, 1); continue; }
        put(s.c, s.r, s.hot ? "*" : ".", s.hot ? 2 : 0, s.life);
      }

      // the ledger itself
      const out = [["LEDGER", `txn ${id}`, 0]];
      txns.forEach((x, i) => { out.push(["PENDING", fmt(x.cents), i]); out.push(["EXPENSE", fmt(-x.cents), i]); });
      text(top, "-".repeat(lw), 2, 0.6);
      out.forEach(([l, r, i], k) => {
        const head = k === 0;
        text(top + 1 + k, line(l, r), head ? 0 : i === 0 ? 1 : 0, head ? 0.9 : 1 - i * 0.28);
      });
      text(top + ledgerH - 1, "-".repeat(lw), 0, 0.5);
      text(top + ledgerH, line("SUM", "0.00" + (Math.floor(t / 500) % 2 ? "_" : " ")), 2, 1);
    },
  };
});
