export const base = import.meta.env.BASE_URL; // "/" locally, "/<repo>/" on Pages
export const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
// brand text may contain `code` spans
export const md = (s) => esc(s).replace(/`([^`]+)`/g, "<code>$1</code>");

export async function copyText(text) {
  try { await navigator.clipboard.writeText(text); return true; }
  catch {
    const t = Object.assign(document.createElement("textarea"), { value: text });
    document.body.append(t); t.select();
    const ok = document.execCommand("copy"); t.remove(); return ok;
  }
}

// Live GitHub facts: cached 1h, never throws, null on any failure (callers show fallbacks).
export async function ghFacts(repo) {
  const key = "gh:" + repo;
  try {
    const hit = JSON.parse(localStorage.getItem(key) || "null");
    if (hit && Date.now() - hit.t < 3600e3) return hit.d;
  } catch {}
  try {
    const [r, rel] = await Promise.all([
      fetch(`https://api.github.com/repos/${repo}`).then((x) => (x.ok ? x.json() : null)),
      fetch(`https://api.github.com/repos/${repo}/releases?per_page=100`).then((x) => (x.ok ? x.json() : [])),
    ]);
    if (!r) return null;
    const downloads = rel.reduce((n, x) => n + x.assets.reduce((m, a) => m + a.download_count, 0), 0);
    const d = { stars: r.stargazers_count, forks: r.forks_count, releases: rel.length, tag: rel[0]?.tag_name ?? null, pre: rel[0]?.prerelease ?? false, downloads, pushed: r.pushed_at };
    try { localStorage.setItem(key, JSON.stringify({ t: Date.now(), d })); } catch {}
    return d;
  } catch { return null; }
}
