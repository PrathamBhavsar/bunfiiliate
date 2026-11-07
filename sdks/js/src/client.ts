import { randomBytes } from "node:crypto";
import { IDEMPOTENCY_KEY_RE, signEvent } from "./signing.ts";
import type { BfEvent, SendResult } from "./types.ts";

export interface ServerOptions {
  baseUrl: string;
  programKey: string;
  secret: string;
  fetch?: typeof fetch;
  /** Base backoff in ms (default 200). Tests set 0. */
  backoffMs?: number;
}

export class BunfiiliateError extends Error {
  status: number;
  code: string;
  constructor(status: number, code: string, message: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

const MAX_RETRIES = 3;

export class BunfiiliateServer {
  private o: ServerOptions;
  constructor(o: ServerOptions) {
    this.o = o;
  }

  async sendEvent(event: Omit<BfEvent, "schema"> & { schema?: "bf.event.v2" }, opts: { idempotencyKey?: string } = {}): Promise<SendResult> {
    if (typeof event.amount_cents === "number" && !Number.isInteger(event.amount_cents)) throw new Error("amount_cents must be an integer");
    const key = opts.idempotencyKey ?? randomBytes(16).toString("hex");
    if (!IDEMPOTENCY_KEY_RE.test(key)) throw new Error("invalid idempotency key");
    const body = JSON.stringify({ schema: "bf.event.v2", ...event });
    const url = `${this.o.baseUrl.replace(/\/+$/, "")}/v2/events/${encodeURIComponent(this.o.programKey)}`;
    const f = this.o.fetch ?? fetch;
    for (let attempt = 0; ; attempt++) {
      // Fresh timestamp per attempt (replay window), same Idempotency-Key.
      const res = await f(url, {
        method: "POST",
        headers: { "content-type": "application/json", "idempotency-key": key, "bf-signature": signEvent(this.o.secret, key, body) },
        body,
      });
      if (res.status === 200 || res.status === 202) return (await res.json()) as SendResult;
      if ((res.status === 429 || res.status === 503) && attempt < MAX_RETRIES) {
        const base = this.o.backoffMs ?? 200;
        const ra = Number(res.headers.get("retry-after"));
        const wait = Math.max(Number.isFinite(ra) ? Math.min(ra, 10) * 1000 * (base ? 1 : 0) : 0, base * 2 ** attempt * (0.5 + Math.random()));
        await new Promise((r) => setTimeout(r, wait));
        continue;
      }
      let code = "http_error", message = `HTTP ${res.status}`;
      try {
        const j = (await res.json()) as { error?: { code?: string; message?: string } };
        code = j.error?.code ?? code;
        message = j.error?.message ?? message;
      } catch {}
      throw new BunfiiliateError(res.status, code, message);
    }
  }
}
