import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { describe, test, type Browser } from '@e2e-dev/web';
import { expect, type TestFixtures } from 'e2e';

// C33: empty / error / forbidden states and an axe smoke. Server answers are route-mocked, so the
// states are deterministic whatever the other suites left in the database.
const env = (name: string) => {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is not set; run this suite through scripts/e2e.sh`);
  return value;
};

const ADMIN = { email: env('E2E_ADMIN_EMAIL'), password: env('E2E_ADMIN_PASSWORD') };

type Fx = Pick<TestFixtures, 'app' | 'screen'> & { browser: Browser };

const AXE = readFileSync(createRequire(import.meta.url).resolve('axe-core/axe.min.js'), 'utf8');

/** Admin list pages: path, heading, list endpoint. */
const LISTS = [
  { path: '/programs', title: 'Programs', api: '/api/v1/programs' },
  { path: '/affiliates', title: 'Affiliates', api: '/api/v1/affiliates' },
  { path: '/referral-codes', title: 'Referral codes', api: '/api/v1/referral-codes' },
  { path: '/conversions', title: 'Conversions', api: '/api/v1/conversions' },
  { path: '/customers', title: 'Customers', api: '/api/v1/customers' },
  { path: '/ledger', title: 'Ledger', api: '/api/v1/ledger/entries' },
  { path: '/payouts', title: 'Payouts', api: '/api/v1/payouts/batches' },
  { path: '/webhooks', title: 'Webhooks', api: '/api/v1/webhooks/endpoints' },
  { path: '/notifications', title: 'Notifications', api: '/api/v1/notifications' },
  { path: '/audit-log', title: 'Audit log', api: '/api/v1/audit-log' },
  { path: '/contact-requests', title: 'Contact requests', api: '/api/v1/contact-requests' },
  { path: '/email-templates', title: 'Email templates', api: '/api/v1/email-templates' },
];

const listUrl = (api: string) => new RegExp(`${api.replaceAll('/', '\\/')}(\\?.*)?$`);

async function signIn({ app, screen, browser }: Fx) {
  await app.open('/login');
  await screen.getByLabel('Email').fill(ADMIN.email);
  await screen.getByLabel('Password').fill(ADMIN.password);
  await screen.getByRole('button', 'Sign in').tap();
  await expect(browser).toHaveURL(/\/dashboard\/?$/);
}

/** Serious or critical axe violations on the current page, as `rule (n nodes)`. */
async function seriousViolations({ browser }: Fx): Promise<string[]> {
  await browser.evaluate(`(() => { (0, eval)(${JSON.stringify(AXE)}); return null; })()`);
  return browser.evaluate(`(async () => {
    const r = await window.axe.run(document, { resultTypes: ['violations'] });
    return r.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical')
      .map((v) => v.id + ' (' + v.nodes.length + ' nodes)');
  })()`);
}

describe('states and a11y', { serial: true }, () => {
  test('sign in', async (f) => {
    await signIn({ app: f.app, screen: f.screen, browser: f.browser });
  });

  test('empty_state_shown_for_new_workspace', async ({ app, screen, browser }) => {
    for (const page of LISTS) {
      await browser.unrouteAll();
      await browser.route(listUrl(page.api), (route) => route.fulfill({ json: { items: [], total: 0 } }));
      await app.open(page.path);
      await expect(screen.getByRole('heading', page.title)).toBeVisible();
      await expect(browser.locator('[data-testid="empty-state"]')).toBeVisible();
    }
    await browser.unrouteAll();
  });

  test('error_state_on_500', async ({ app, screen, browser }) => {
    let calls = 0;
    await browser.route(listUrl('/api/v1/customers'), (route) => {
      calls += 1;
      return calls === 1
        ? route.fulfill({ status: 500, json: { error: { code: 'internal', message: 'Database is on fire' } } })
        : route.continue();
    });
    await app.open('/customers');
    const state = browser.locator('[data-testid="error-state"]');
    await expect(state).toContainText('Database is on fire');
    // The server message, never raw JSON.
    await expect(state).not.toContainText('"error"');
    await state.getByRole('button', 'Retry').tap();
    await expect(state).toBeHidden();
    expect(calls).toBeGreaterThan(1);
    await browser.unrouteAll();
  });

  test('forbidden_state_on_403', async ({ app, browser }) => {
    await browser.route(listUrl('/api/v1/customers'), (route) =>
      route.fulfill({ status: 403, json: { error: { code: 'forbidden', message: 'insufficient role' } } }),
    );
    await app.open('/customers');
    const state = browser.locator('[data-testid="forbidden-state"]');
    await expect(state).toContainText('No access');
    await expect(state.getByRole('button', 'Retry')).toHaveCount(0);
    await browser.unrouteAll();
  });

  test('keyboard_tab_reaches_primary_nav', async ({ app, screen, browser }) => {
    await app.open('/dashboard');
    await expect(screen.getByRole('heading', 'Dashboard')).toBeVisible();
    const nav = browser.locator('nav[aria-label="Main"]');
    await expect(nav).toBeVisible();
    // Tab until focus lands inside the main navigation; a bounded walk, not a fixed key count.
    let inNav = false;
    for (let i = 0; i < 30 && !inNav; i++) {
      await browser.keyboard.press('Tab');
      inNav = await browser.evaluate(`document.activeElement?.closest('nav[aria-label="Main"]') !== null`);
    }
    expect(inNav).toBe(true);
  });

  for (const page of ['/dashboard', '/programs', '/affiliates', '/referral-codes', '/payouts']) {
    test(`axe_no_serious_violations_${page.slice(1)}`, async (f) => {
      await f.app.open(page);
      await expect(f.browser.locator('#page-title')).toBeVisible();
      expect(await seriousViolations({ app: f.app, screen: f.screen, browser: f.browser })).toEqual([]);
    });
  }

  test('axe_no_serious_violations_login', async (f) => {
    await f.screen.getByRole('button', 'Sign out').tap();
    await expect(f.browser).toHaveURL(/\/login\/?$/);
    expect(await seriousViolations({ app: f.app, screen: f.screen, browser: f.browser })).toEqual([]);
  });
});
