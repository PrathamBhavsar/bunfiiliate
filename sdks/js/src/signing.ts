import { createHmac, timingSafeEqual } from "node:crypto";

export const IDEMPOTENCY_KEY_RE = /^[A-Za-z0-9_-]{1,64}$/;

function hmacHex(secret: string, payload: string): string {
  return createHmac("sha256", secret).update(payload).digest("hex");
}

/** `Bf-Signature` value for POST /v2/events/{key}: HMAC-SHA256 over `{t}.{key}.{body}`. */
export function signEvent(secret: string, idempotencyKey: string, rawBody: string, t: number = Math.floor(Date.now() / 1000)): string {
  if (!IDEMPOTENCY_KEY_RE.test(idempotencyKey)) throw new Error("invalid idempotency key");
  return `t=${t},v1=${hmacHex(secret, `${t}.${idempotencyKey}.${rawBody}`)}`;
}

type HeaderBag = Headers | Record<string, string | string[] | undefined>;

function header(headers: HeaderBag, name: string): string | undefined {
  if (typeof (headers as Headers).get === "function") return (headers as Headers).get(name) ?? undefined;
  for (const [k, v] of Object.entries(headers as Record<string, string | string[] | undefined>)) {
    if (k.toLowerCase() === name) return Array.isArray(v) ? v.join(",") : v;
  }
  return undefined;
}

/** Verify an outbound v2 webhook (`Bf-Signature`, signed over `{t}.{body}`). Any `v1=` may match. */
export function verifyOutbound(secret: string, headers: HeaderBag, rawBody: string, toleranceSecs = 300, now: number = Math.floor(Date.now() / 1000)): boolean {
  const sig = header(headers, "bf-signature");
  if (!sig) return false;
  let t: number | undefined;
  const macs: string[] = [];
  for (const part of sig.split(",")) {
    const i = part.indexOf("=");
    const k = part.slice(0, i).trim();
    const v = part.slice(i + 1).trim();
    if (k === "t" && /^\d{1,15}$/.test(v)) t = Number(v);
    else if (k === "v1") macs.push(v);
  }
  if (t === undefined || macs.length === 0 || Math.abs(now - t) > toleranceSecs) return false;
  const want = Buffer.from(hmacHex(secret, `${t}.${rawBody}`), "hex");
  let ok = false;
  for (const m of macs) {
    const got = Buffer.from(m, "hex");
    if (got.length === want.length && timingSafeEqual(got, want)) ok = true;
  }
  return ok;
}
