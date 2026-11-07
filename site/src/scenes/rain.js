// Click rain: drops fall, the ones that land on the ledger post a balanced two-line transaction. SUM stays 0.00.
import { registerScene } from "../kit/ascii.js";

const W = 34;
const fmt = (cents) => (cents < 0 ? "-" : "+") + (Math.abs(cents) / 100).toFixed(2);
const line = (l, r) => l + " ".repeat(Math.max(1, W - l.length - r.length)) + r;

registerScene("rain", ({ cols, rows, rand }) => {
  const wide = cols >= 72;
  const lw = Math.min(W, cols - 2);
  const x0 = wide ? cols - lw - 3 : Math.floor((cols - lw) / 2);
  const ledgerH = 10;
  const top = rows - ledgerH - 1;
  const drops = [], splashes = [], txns = [];
  let acc = 0, id = 4800 + Math.floor(rand() * 90);

  const spawn = () => drops.push({ x: Math.floor(rand() * cols), y: -2, v: 0.006 + rand() * 0.009, len: 4 + Math.floor(rand() * 6) });
  for (let i = 0; i < cols * 0.6; i++) { spawn(); drops[i].y = rand() * rows; }

  const post = () => {
    const cents = 100 + Math.floor(rand() * 9900);
    txns.unshift({ id: ++id, cents, born: 0 });
    if (txns.length > 3) txns.pop();
  };

  return {
    step(t, dt, ptr, put) {
      acc += dt * cols * 0.00014;
      while (acc >= 1) { spawn(); acc -= 1; }
      for (let i = drops.length - 1; i >= 0; i--) {
        const d = drops[i];
        d.y += d.v * dt;
        const dx = d.x - ptr.col, dy = d.y - ptr.row;
        if (Math.abs(dx) < 6 && Math.abs(dy) < 8) d.x += Math.sign(dx || 1) * 0.12;
        const c = Math.round(d.x);
        const inBand = c >= x0 && c < x0 + lw;
        const floor = inBand ? top - 1 : rows - 1;
        if (d.y >= floor) {
          if (inBand) { post(); splashes.push({ c, r: floor, life: 1, hot: true }); }
          else splashes.push({ c, r: rows - 1, life: 0.7, hot: false });
          drops.splice(i, 1);
          continue;
        }
        const r = Math.floor(d.y);
        put(c, r, "*", 2, 1);
        for (let k = 1; k < d.len; k++) put(c, r - k, k < 3 ? "+" : k < 5 ? ":" : ".", 0, 1 - k / d.len);
      }
      for (let i = splashes.length - 1; i >= 0; i--) {
        const s = splashes[i]; s.life -= dt * 0.0016;
        if (s.life <= 0) { splashes.splice(i, 1); continue; }
        put(s.c, s.r, s.hot ? "*" : ".", s.hot ? 2 : 0, s.life);
      }

      // ledger floor
      const rowsOut = [["LEDGER", `txn ${id}`, 0]];
      txns.forEach((x, i) => {
        rowsOut.push([`PENDING`, fmt(x.cents), i]);
        rowsOut.push([`EXPENSE`, fmt(-x.cents), i]);
      });
      const text = (r, s, tone, a) => { for (let i = 0; i < s.length; i++) put(x0 + i, r, s[i], tone, a, true); };
      text(top, "-".repeat(lw), 0, 0.5);
      rowsOut.forEach(([l, rr, i], k) => {
        const head = k === 0;
        text(top + 1 + k, line(l, rr).slice(0, lw), head ? 0 : i === 0 ? 1 : 0, head ? 0.8 : 1 - i * 0.28);
      });
      text(top + ledgerH - 1, "-".repeat(lw), 0, 0.5);
      text(top + ledgerH, line("SUM", "0.00" + (Math.floor(t / 500) % 2 ? "_" : " ")).slice(0, lw), 2, 1);
    },
  };
});
