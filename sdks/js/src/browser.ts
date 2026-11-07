/** Browser helper: thin wrapper over the `/t.js` v2 script (window.bunfiiliate). */
interface Win { bunfiiliate?: { track(code?: string, click?: string): void; click_id(): string | null } }

export function init(opts: { pk?: string; endpoint?: string } = {}): void {
  const d = (globalThis as any).document;
  if (!d || (globalThis as Win).bunfiiliate) return;
  const s = d.createElement("script");
  const base = (opts.endpoint ?? "").replace(/\/+$/, "");
  s.src = `${base}/t.js`;
  s.async = true;
  if (opts.pk) s.setAttribute("data-pk", opts.pk);
  d.head.appendChild(s);
}

export function track(code?: string, click?: string): void {
  (globalThis as Win).bunfiiliate?.track(code, click);
}

export function click_id(): string | null {
  return (globalThis as Win).bunfiiliate?.click_id() ?? null;
}
