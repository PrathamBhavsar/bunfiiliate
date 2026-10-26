import { createHmac } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { describe, test, type Browser } from '@e2e-dev/web';
import { expect, type TestFixtures } from 'e2e';

// Click-to-cash through the real UI and tracker, one scratch database, deterministic steps only.
// scripts/e2e.sh starts the stack and passes the env below.
const env = (name: string) => {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is not set; run this suite through scripts/e2e.sh`);
  return value;
};

const APP = env('E2E_APP_URL');
const ADMIN = { email: env('E2E_ADMIN_EMAIL'), password: env('E2E_ADMIN_PASSWORD') };
const WORKSPACE = env('E2E_WORKSPACE');

const AFFILIATE = { email: 'affiliate@e2e.test', password: 'e2e-affiliate-pw', payout: 'payouts@e2e.test' };
const PROGRAM = 'E2E Program';
const CODE = 'e2e-code';
const SALE_ID = 'e2e-sale-1';
const BAD_SALE_ID = 'e2e-bad-sale';
// 5000 cents at 2000 bps (20%).
const SALE_CENTS = 5000;
const SALE = '50.00 USD';
const COMMISSION = '10.00 USD';

type Fx = Pick<TestFixtures, 'app' | 'screen'> & { browser: Browser };

const state = { publicKey: '', webhookSecret: '', clickId: '' };

const dialog = ({ screen }: Fx, name: string | RegExp) => screen.getByRole('dialog', name);
// A titled block of a detail sheet (`Section` in admin-ui).
const section = ({ browser }: Fx, title: string) => browser.locator(`section:has(> div > h3:text-is("${title}"))`);

async function signIn(fx: Fx, user: { email: string; password: string }, home: RegExp) {
  await fx.app.open('/login');
  await fx.screen.getByLabel('Email').fill(user.email);
  await fx.screen.getByLabel('Password').fill(user.password);
  await fx.screen.getByRole('button', 'Sign in').tap();
  await expect(fx.browser).toHaveURL(home);
}

async function signOut(fx: Fx) {
  // An open detail sheet hides the header from the accessibility tree: start from a page without one.
  await fx.app.open('/notifications');
  await fx.screen.getByRole('button', 'Sign out').tap();
  await expect(fx.browser).toHaveURL(/\/login\/?$/);
}

/** Open a list page and its detail sheet for the row containing `text`. */
async function openRow(fx: Fx, path: string, title: string, text: string) {
  await fx.app.open(path);
  await expect(fx.screen.getByRole('heading', title)).toBeVisible();
  await fx.screen.getByRole('row').filter({ hasText: text }).tap();
}

/** Lists do not live-update: reload until the row appears. */
async function waitForRow(fx: Fx, text: string) {
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
async function confirm(fx: Fx, trigger: string, dialogTitle: string | RegExp, confirmLabel: string) {
  await fx.screen.getByRole('button', trigger).tap();
  await dialog(fx, dialogTitle).getByRole('button', confirmLabel).tap();
  await expect(dialog(fx, dialogTitle)).toBeHidden();
}

// Signs `<timestamp>.<idempotency key>.<body>`; the tracker also checks the timestamp is fresh.
function signedHeaders(secret: string, idempotencyKey: string, body: string) {
  const ts = String(Math.floor(Date.now() / 1000));
  const signature = createHmac('sha256', secret).update(`${ts}.${idempotencyKey}.${body}`).digest('hex');
  return { 'content-type': 'application/json', 'idempotency-key': idempotencyKey, 'x-bf-timestamp': ts, 'x-bf-signature': signature };
}

function postEvent(key: string, secret: string, idempotencyKey: string, body: string) {
  return fetch(`${APP}/v1/events/${key}`, {
    method: 'POST',
    headers: signedHeaders(secret, idempotencyKey, body),
    body,
  });
}

describe('affiliate click to cash', { serial: true }, () => {
  test('admin creates a program with a site and a commission plan', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
    await signIn(fx, ADMIN, /\/dashboard\/?$/);

    await app.open('/programs');
    await screen.getByRole('button', 'New program').tap();
    await dialog(fx, 'New program').getByLabel('Name').fill(PROGRAM);
    await dialog(fx, 'New program').getByLabel('Currency').fill('USD');
    await dialog(fx, 'New program').getByRole('button', 'Create').tap();
    await expect(dialog(fx, 'New program')).toBeHidden();

    await screen.getByRole('row').filter({ hasText: PROGRAM }).tap();

    await section(fx, 'Sites').getByRole('button', 'Add').tap();
    await dialog(fx, 'Add site').getByLabel('Domain').fill('localhost');
    await dialog(fx, 'Add site').getByRole('button', 'Save').tap();
    await expect(section(fx, 'Sites')).toContainText('localhost');

    await section(fx, 'Commission plans').getByRole('button', 'Add').tap();
    await dialog(fx, 'Add commission plan').getByLabel('Rate (bps)').fill('2000');
    await dialog(fx, 'Add commission plan').getByRole('button', 'Save').tap();
    await expect(section(fx, 'Commission plans')).toContainText('20.00% · lifetime');

    await section(fx, 'Settings').getByRole('button', 'Edit').tap();
    await dialog(fx, 'Edit program').getByLabel('Default plan').selectOption('20.00% · lifetime');
    await dialog(fx, 'Edit program').getByRole('button', 'Save').tap();
    await expect(section(fx, 'Commission plans')).toContainText('default');

    await section(fx, 'Keys and secrets').getByRole('button', 'Reveal').tap();
    state.publicKey = ((await browser.locator('dt:text-is("Public key") + dd').textContent()) ?? '').trim();
    state.webhookSecret = ((await browser.locator('dt:text-is("Webhook secret") + dd').textContent()) ?? '').trim();
    expect(state.publicKey).toMatch(/^\S+$/);
    expect(state.webhookSecret).not.toContain('•');
    expect(state.webhookSecret.length).toBeGreaterThan(8);
  });

  test('a visitor registers as an affiliate and the admin approves and enrolls them', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
    await signOut(fx);

    await app.open('/register');
    await screen.getByLabel('Workspace').fill(WORKSPACE);
    await screen.getByLabel('Email').fill(AFFILIATE.email);
    await screen.getByLabel('Password').fill(AFFILIATE.password);
    await screen.getByLabel('Payout email (optional)').fill(AFFILIATE.payout);
    await screen.getByRole('button', 'Create account').tap();
    await expect(screen.getByRole('status')).toContainText('pending', { ignoreCase: true });

    await signIn(fx, ADMIN, /\/dashboard\/?$/);
    await openRow(fx, '/affiliates', 'Affiliates', AFFILIATE.email);
    await confirm(fx, 'Approve', `Approve ${AFFILIATE.email}?`, 'Approve');
    await expect(section(fx, 'Profile')).toContainText('active', { ignoreCase: true });

    await openRow(fx, '/programs', 'Programs', PROGRAM);
    await section(fx, 'Enrolled affiliates').getByRole('button', 'Enroll').tap();
    await dialog(fx, 'Enroll affiliate').getByLabel('Affiliate').selectOption(AFFILIATE.email);
    await dialog(fx, 'Enroll affiliate').getByRole('button', 'Save').tap();
    await expect(section(fx, 'Enrolled affiliates')).toContainText(AFFILIATE.email);
  });

  test('the affiliate creates a referral code and a visit is tracked', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
    await signOut(fx);
    await signIn(fx, AFFILIATE, /\/me\/?$/);

    await app.open('/me/links');
    await screen.getByRole('button', 'New link').tap();
    await dialog(fx, 'New link').getByLabel('Program').selectOption(PROGRAM);
    await dialog(fx, 'New link').getByLabel('Code').fill(CODE);
    await dialog(fx, 'New link').getByLabel('Landing URL').fill(`${APP}/register`);
    await dialog(fx, 'New link').getByRole('button', 'Create').tap();
    await expect(screen.getByRole('row').filter({ hasText: CODE })).toHaveCount(1);

    // The tracker answers /r/<code> with a 302 to the landing page carrying the click id.
    await browser.goto(`/r/${CODE}`);
    await expect(browser).toHaveURL(/\/register\/?\?bf_click=[0-9a-f-]{36}$/);
    state.clickId = new URL(await browser.url()).searchParams.get('bf_click') ?? '';
    expect(state.clickId).toHaveLength(36);
  });

  test('a signed conversion is ingested, a badly signed one is refused', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };

    const forged = JSON.stringify({ customer: 'cust-bad', amount_cents: SALE_CENTS, currency: 'USD', click: state.clickId });
    const bad = await fetch(`${APP}/v1/events/${state.publicKey}`, {
      method: 'POST',
      headers: signedHeaders('not-the-secret', BAD_SALE_ID, forged),
      body: forged,
    });
    expect(bad.status).toBe(400);

    // Only the click id ties the sale to the affiliate: no referral code in the payload.
    const body = JSON.stringify({ customer: 'cust-1', amount_cents: SALE_CENTS, currency: 'USD', click: state.clickId });
    const good = await postEvent(state.publicKey, state.webhookSecret, SALE_ID, body);
    expect(good.status).toBe(202);

    await signOut(fx);
    await signIn(fx, ADMIN, /\/dashboard\/?$/);
    await app.open('/conversions');
    await waitForRow(fx, SALE_ID);

    const row = screen.getByRole('row').filter({ hasText: SALE_ID });
    await expect(row).toContainText(AFFILIATE.email);
    await expect(row).toContainText(SALE);
    await expect(row).toContainText(COMMISSION);
    await expect(row).toContainText('pending', { ignoreCase: true });
    // The refused webhook left nothing behind.
    await expect(screen.getByText(BAD_SALE_ID)).toHaveCount(0);
    await expect(browser.locator('tbody tr')).toHaveCount(1);
  });

  test('admin approves the conversion and pays it out', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };

    await openRow(fx, '/conversions', 'Conversions', SALE_ID);
    await confirm(fx, 'Approve', `Approve conversion ${SALE_ID}?`, 'Approve');
    await expect(section(fx, 'Sale')).toContainText('approved', { ignoreCase: true });

    await app.open('/payouts');
    await screen.getByRole('button', 'New batch').tap();
    await dialog(fx, 'New payout batch').getByLabel('Program').selectOption(PROGRAM);
    await dialog(fx, 'New payout batch').getByRole('button', 'Create draft').tap();
    const batch = dialog(fx, /^Batch /);
    await expect(batch).toBeVisible();
    await expect(section(fx, 'Batch')).toContainText('draft', { ignoreCase: true });
    await expect(section(fx, 'Payouts')).toContainText(AFFILIATE.payout);

    await confirm(fx, 'Approve', 'Approve?', 'Approve');
    await expect(section(fx, 'Batch')).toContainText('approved', { ignoreCase: true });
    await confirm(fx, 'Mark paid', 'Mark paid?', 'Mark paid');
    await expect(section(fx, 'Batch')).toContainText('paid', { ignoreCase: true });

    const file = await browser.waitForDownload(() => screen.getByRole('button', 'Download CSV').tap());
    const csv = await readFile(file.absolutePath, 'utf8');
    expect(csv).toBe(`email,amount,currency\n${AFFILIATE.payout},10.00,USD\n`);
  });

  test('the affiliate sees the paid commission in their earnings', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
    await signOut(fx);
    await signIn(fx, AFFILIATE, /\/me\/?$/);

    await app.open('/me/earnings');
    const row = screen.getByRole('row').filter({ hasText: SALE });
    await expect(row).toHaveCount(1);
    await expect(row).toContainText(COMMISSION);
    await expect(row).toContainText('paid', { ignoreCase: true });
  });
});
