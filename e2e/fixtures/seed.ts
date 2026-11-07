import { createHmac, randomBytes } from 'node:crypto';
import { expect, type TestFixtures } from 'e2e';
import type { Browser } from '@e2e-dev/web';

// Shared by every suite: env, UI helpers and an API client that seeds each file's own data, so a
// file passes alone, in any order, and any number of times. Names carry a per-run suffix.
export const env = (name: string) => {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is not set; run this suite through scripts/e2e.sh`);
  return value;
};

export const APP = env('E2E_APP_URL');
export const ADMIN = { email: env('E2E_ADMIN_EMAIL'), password: env('E2E_ADMIN_PASSWORD') };
export const WORKSPACE = env('E2E_WORKSPACE');
// Seeding talks to the api directly: it sees no proxy header, so only the per-email limits apply
// (the browser, through Caddy, shares one per-IP login budget across all suites).
const API = process.env.E2E_API_URL ?? APP;
const API_ORIGIN = new URL(API).origin;

export type Fx = Pick<TestFixtures, 'app' | 'screen'> & { browser: Browser };
export type Creds = { email: string; password: string };

/** Short unique token: run time plus randomness, safe in slugs, emails and codes. */
export const unique = () => `${Date.now().toString(36)}${randomBytes(2).toString('hex')}`;

export const dialog = ({ screen }: Fx, name: string | RegExp) => screen.getByRole('dialog', name);
/** A titled block of a detail sheet (`Section` in admin-ui). */
export const section = ({ browser }: Fx, title: string) => browser.locator(`section:has(> div > h3:text-is("${title}"))`);
export const listUrl = (api: string) => new RegExp(`${api.replaceAll('/', '\\/')}(\\?.*)?$`);

export async function signIn(fx: Fx, user: Creds, home: RegExp) {
  await fx.app.open('/login');
  await fx.screen.getByLabel('Email').fill(user.email);
  await fx.screen.getByLabel('Password').fill(user.password);
  await fx.screen.getByRole('button', 'Sign in').tap();
  await expect(fx.browser).toHaveURL(home);
}

export const signInAdmin = (fx: Fx, user: Creds = ADMIN) => signIn(fx, user, /\/dashboard\/?$/);
export const signInAffiliate = (fx: Fx, user: Creds) => signIn(fx, user, /\/me\/?$/);

export async function signOut(fx: Fx) {
  // An open detail sheet hides the header from the accessibility tree: start from a page without one.
  await fx.app.open('/notifications');
  await fx.screen.getByRole('button', 'Sign out').tap();
  await expect(fx.browser).toHaveURL(/\/login\/?$/);
}

/** Open a list page and its detail sheet for the row containing `text`. */
export async function openRow(fx: Fx, path: string, title: string, text: string) {
  await fx.app.open(path);
  await expect(fx.screen.getByRole('heading', title)).toBeVisible();
  await fx.screen.getByRole('row').filter({ hasText: text }).tap();
}

/** Lists do not live-update: reload until the row appears. */
export async function waitForRow(fx: Fx, text: string) {
  await expect
    .poll(
      async () => {
        await fx.browser.reload();
        return fx.screen.getByRole('row').filter({ hasText: text }).count();
      },
      { timeout: 45_000, interval: 1_000 },
    )
    .toBe(1);
}

/** Click a confirm-dialog trigger in the open sheet, then its confirm button. */
export async function confirm(fx: Fx, trigger: string, dialogTitle: string | RegExp, confirmLabel: string) {
  await fx.screen.getByRole('button', trigger).tap();
  await dialog(fx, dialogTitle).getByRole('button', confirmLabel).tap();
  await expect(dialog(fx, dialogTitle)).toBeHidden();
}

// Signs `<timestamp>.<idempotency key>.<body>`; the tracker also checks the timestamp is fresh.
export function signedHeaders(secret: string, idempotencyKey: string, body: string) {
  const ts = String(Math.floor(Date.now() / 1000));
  const signature = createHmac('sha256', secret).update(`${ts}.${idempotencyKey}.${body}`).digest('hex');
  return { 'content-type': 'application/json', 'idempotency-key': idempotencyKey, 'x-bf-timestamp': ts, 'x-bf-signature': signature };
}

export function postEvent(key: string, secret: string, idempotencyKey: string, body: string) {
  return fetch(`${APP}/v1/events/${key}`, { method: 'POST', headers: signedHeaders(secret, idempotencyKey, body), body });
}

/** Cookie-session API client with the double-submit CSRF header and a same-origin `Origin`. */
export class Api {
  private jar = new Map<string, string>();

  private store(res: Response) {
    for (const line of res.headers.getSetCookie()) {
      const [pair] = line.split(';');
      const eq = pair.indexOf('=');
      this.jar.set(pair.slice(0, eq).trim(), pair.slice(eq + 1));
    }
  }

  async call(method: string, path: string, body?: unknown): Promise<Response> {
    const headers: Record<string, string> = { origin: API_ORIGIN, cookie: [...this.jar].map(([k, v]) => `${k}=${v}`).join('; ') };
    if (method !== 'GET') {
      if (!this.jar.has('bf_csrf')) this.store(await fetch(`${API}/api/v1/auth/csrf`, { headers: { origin: API_ORIGIN } }));
      headers['x-csrf-token'] = this.jar.get('bf_csrf') ?? '';
      headers.cookie = [...this.jar].map(([k, v]) => `${k}=${v}`).join('; ');
    }
    if (body !== undefined) headers['content-type'] = 'application/json';
    const res = await fetch(`${API}${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
    this.store(res);
    return res;
  }

  /** JSON of a call that must have succeeded; `null` for an empty body. */
  async ok<T = any>(method: string, path: string, body?: unknown): Promise<T> {
    const res = await this.call(method, path, body);
    const text = await res.text();
    if (!res.ok) throw new Error(`${method} ${path} -> ${res.status} ${text}`);
    return (text ? JSON.parse(text) : null) as T;
  }

  async login(user: Creds): Promise<this> {
    await this.ok('POST', '/api/v1/auth/login', user);
    return this;
  }
}

export type World = {
  suffix: string;
  /** The world's own workspace: its slug and admin (signed in on `admin`). */
  ws: { slug: string; admin: Creds };
  program: { id: string; name: string; publicKey: string; webhookSecret: string };
  affiliate: Creds & { id: string; payout: string };
  code: string;
  admin: Api;
  aff: Api;
};

/** A fresh workspace (self-serve signup, so the stack runs with `SIGNUP_ENABLED=1`) and its signed-in admin. */
export async function seedWorkspace(): Promise<{ slug: string; creds: Creds; admin: Api }> {
  const suffix = unique();
  const slug = `ws-${suffix}`;
  const creds = { email: `admin-${suffix}@e2e.test`, password: 'e2e-ws-admin-password' };
  const admin = new Api();
  await admin.ok('POST', '/api/v1/signup', { workspace_name: slug, admin_email: creds.email, admin_password: creds.password });
  return { slug, creds, admin };
}

/** An approved, enrolled affiliate with a login. */
export async function seedAffiliate(admin: Api, slug: string, programId: string, tag: string) {
  const creds = { email: `aff-${tag}@e2e.test`, password: 'e2e-affiliate-pw' };
  const payout = `pay-${tag}@e2e.test`;
  const reg = await new Api().ok('POST', '/api/v1/affiliates/register', { workspace: slug, ...creds, payout_email: payout });
  await admin.ok('POST', `/api/v1/affiliates/${reg.id}/approve`);
  await admin.ok('POST', `/api/v1/programs/${programId}/enrollments`, { affiliate_id: reg.id });
  return { ...creds, id: reg.id as string, payout };
}

/**
 * Own workspace with one program (USD, 20% default plan, site `localhost`) and one active, enrolled
 * affiliate that owns one referral code. Everything carries a fresh suffix, so suites never share rows.
 */
export async function seedWorld(): Promise<World> {
  const ws = await seedWorkspace();
  const suffix = ws.slug.slice(3);
  const admin = ws.admin;
  const name = `Program ${suffix}`;
  const program = await admin.ok('POST', '/api/v1/programs', { name, currency: 'USD' });
  await admin.ok('POST', `/api/v1/programs/${program.id}/sites`, { domain: 'localhost' });
  const plan = await admin.ok('POST', `/api/v1/programs/${program.id}/plans`, { kind: 'percent', duration: 'lifetime', rate_bps: 2000 });
  await admin.ok('PATCH', `/api/v1/programs/${program.id}`, { default_plan_id: plan.id });
  const affiliate = await seedAffiliate(admin, ws.slug, program.id, suffix);
  const aff = await new Api().login(affiliate);
  const code = `c-${suffix}`;
  await aff.ok('POST', '/api/v1/me/referral-codes', { program_id: program.id, code, landing_url: `${APP}/register` });
  return {
    suffix,
    ws: { slug: ws.slug, admin: ws.creds },
    program: { id: program.id, name, publicKey: program.public_key, webhookSecret: program.webhook_secret },
    affiliate,
    code,
    admin,
    aff,
  };
}

/** Follow `/r/<code>` and return the click id the tracker put on the redirect (the snapshot reloads shortly after a code is created). */
export async function click(code: string): Promise<string> {
  let id = '';
  for (let i = 0; i < 40 && !id; i++) {
    const res = await fetch(`${APP}/r/${code}`, { redirect: 'manual' });
    id = new URL(res.headers.get('location') ?? '', APP).searchParams.get('bf_click') ?? '';
    if (!id) await new Promise((r) => setTimeout(r, 250));
  }
  expect(id).toHaveLength(36);
  return id;
}

/** Ingest one signed sale for a click; 202 or throws. */
export async function sale(w: World, clickId: string, id: string, cents = 5000) {
  const body = JSON.stringify({ customer: `cust-${id}`, amount_cents: cents, currency: 'USD', click: clickId });
  const res = await postEvent(w.program.publicKey, w.program.webhookSecret, id, body);
  expect(res.status).toBe(202);
}
