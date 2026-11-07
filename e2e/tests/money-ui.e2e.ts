import { describe, test, type Browser } from '@e2e-dev/web';
import { expect, type TestFixtures } from 'e2e';

// B01 webhook tester, B03 payout request page, B04 rail selector. The tester talks to the real
// api and tracker; the payout pages are route-mocked so batch and balance state is deterministic.
const env = (name: string) => {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is not set; run this suite through scripts/e2e.sh`);
  return value;
};

const ADMIN = { email: env('E2E_ADMIN_EMAIL'), password: env('E2E_ADMIN_PASSWORD') };
type Fx = Pick<TestFixtures, 'app' | 'screen'> & { browser: Browser };

const dialog = ({ screen }: Fx, name: string | RegExp) => screen.getByRole('dialog', name);
const url = (api: string) => new RegExp(`${api.replaceAll('/', '\\/')}(\\?.*)?$`);

const SUFFIX = Date.now().toString(36);
const PROGRAM = `Tester ${SUFFIX}`;
const BATCH_ID = '00000000-0000-4000-8000-0000000000b1';
const batch = {
  id: BATCH_ID,
  program_id: '00000000-0000-4000-8000-0000000000a1',
  status: 'approved',
  total_cents: 1000,
  currency: 'USD',
  payout_count: 1,
  created_at: '2026-01-01T00:00:00Z',
  created_by: '00000000-0000-4000-8000-0000000000c1',
  approved_by: '00000000-0000-4000-8000-0000000000c1',
  paid_at: null,
};

describe('money UI', { serial: true }, () => {
  test('sign in', async ({ app, screen, browser }) => {
    await app.open('/login');
    await screen.getByLabel('Email').fill(ADMIN.email);
    await screen.getByLabel('Password').fill(ADMIN.password);
    await screen.getByRole('button', 'Sign in').tap();
    await expect(browser).toHaveURL(/\/dashboard\/?$/);
  });

  test('webhook tester sends a signed test event and shows the raw answer', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
    await app.open('/programs');
    await screen.getByRole('button', 'New program').tap();
    await dialog(fx, 'New program').getByLabel('Name').fill(PROGRAM);
    await dialog(fx, 'New program').getByLabel('Currency').fill('USD');
    await dialog(fx, 'New program').getByRole('button', 'Create').tap();
    await expect(dialog(fx, 'New program')).toBeHidden();

    await app.open('/webhooks/test');
    await expect(screen.getByRole('heading', 'Webhook tester')).toBeVisible();
    await screen.getByLabel('Test program').selectOption(PROGRAM);
    // The payload is prefilled per source.
    await expect(screen.getByLabel('JSON payload')).toHaveValue(/"amount_cents"/);
    await screen.getByLabel('Event source').selectOption('stripe');
    await expect(screen.getByLabel('JSON payload')).toHaveValue(/charge\.refunded/);
    await screen.getByLabel('Event source').selectOption('generic');

    // Bad JSON never leaves the browser.
    await screen.getByLabel('JSON payload').fill('{ nope');
    await screen.getByRole('button', 'Send test').tap();
    await expect(browser.locator('form [role="alert"]')).toContainText('not valid JSON');

    await screen.getByLabel('JSON payload').fill(JSON.stringify({ type: 'sale', customer: 'cus_1', amount_cents: 4900, currency: 'USD' }));
    await screen.getByRole('button', 'Send test').tap();
    // Whatever the tracker answers (accepted or refused) is shown verbatim with its HTTP status.
    await expect(browser.locator('[data-testid="test-status"]')).toContainText(/HTTP \d{3}/);
  });

  test('webhook replay re-queues a processed event', async ({ app, screen, browser }) => {
    const id = '00000000-0000-4000-8000-0000000000e1';
    const row = {
      id,
      program_id: batch.program_id,
      source: 'generic',
      external_id: 'evt-replay-1',
      payload: {},
      received_at: '2026-01-01T00:00:00Z',
      processed_at: '2026-01-01T00:00:05Z',
      error: 'boom',
    };
    let replayed = 0;
    await browser.route(url('/api/v1/webhooks/inbound'), (route) => route.fulfill({ json: { items: [row], total: 1 } }));
    await browser.route(/\/api\/v1\/webhooks\/inbound\/[^/]+\/replay$/, (route) => {
      replayed += 1;
      return route.fulfill({ json: { ...row, processed_at: null, error: null } });
    });
    await app.open('/webhooks/test');
    const line = screen.getByRole('row').filter({ hasText: 'evt-replay-1' });
    await expect(line).toContainText('boom');
    await line.getByRole('button', 'Replay').tap();
    await expect.poll(() => replayed).toBe(1);
    await browser.unrouteAll();
  });

  test('payout rail selector offers only available rails and shows the outcome', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
    let body = '';
    await browser.route(url('/api/v1/payouts/batches'), (route) => route.fulfill({ json: { items: [batch], total: 1 } }));
    await browser.route(/\/api\/v1\/payouts\/batches\/[^/]+$/, (route) => route.fulfill({ json: { ...batch, payouts: [] } }));
    await browser.route(url('/api/v1/payouts/rails'), (route) =>
      route.fulfill({
        json: [
          { rail: 'manual_csv', available: true, configured: false, sandbox: null },
          { rail: 'mock', available: true, configured: false, sandbox: null },
          { rail: 'wise', available: false, configured: false, sandbox: null },
        ],
      }),
    );
    await browser.route(/\/api\/v1\/payouts\/batches\/[^/]+\/mark-paid$/, (route) => {
      body = route.request.postData ?? '';
      return route.fulfill({ json: { ...batch, status: 'approved', payouts: [], outcome: { paid: 0, submitted: 1, failed: 0, batch_paid: false } } });
    });
    await app.open(`/payouts?id=${BATCH_ID}`);
    await screen.getByRole('button', 'Mark paid').tap();
    const d = dialog(fx, 'Mark paid?');
    await expect(d.getByLabel('Payout rail')).toContainText('Mock (testing)');
    await expect(d.getByLabel('Payout rail')).not.toContainText('Wise');
    await d.getByLabel('Payout rail').selectOption('Mock (testing)');
    await d.getByRole('button', 'Mark paid').tap();
    await expect(dialog(fx, 'Mark paid?')).toBeHidden();
    await expect(browser.locator('[data-testid="rail-outcome"]')).toContainText('1 submitted');
    expect(JSON.parse(body)).toEqual({ rail: 'mock' });
    await browser.unrouteAll();
  });
});
