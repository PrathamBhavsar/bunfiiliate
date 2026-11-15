import { describe, test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { dialog, seedWorld, signInAffiliate, unique, type World } from '../fixtures/seed.js';

// Affiliate portal: links, tracked visit, every /me page, payout request form.
let world: Promise<World> | undefined;
const w = () => (world ??= seedWorld());

const ME_PAGES: [string, string][] = [
  ['/me', 'Dashboard'],
  ['/me/links', 'Links'],
  ['/me/earnings', 'Earnings'],
  ['/me/payouts', 'Payouts'],
  ['/me/profile', 'Profile'],
];

describe('affiliate portal', { serial: true }, () => {
  test('sign in', async (f) => {
    await signInAffiliate(f, (await w()).affiliate);
  });

  test('every portal page renders its heading', async ({ app, screen, browser }) => {
    for (const [path, title] of ME_PAGES) {
      await app.open(path);
      await expect(screen.getByRole('heading', title).first()).toBeVisible();
      await expect(browser.locator('main')).toBeVisible();
    }
  });

  test('the affiliate creates a referral code and a visit is tracked', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
    const { program, suffix } = await w();
    const code = `ui-${suffix}${unique().slice(-3)}`;
    await app.open('/me/links');
    await screen.getByRole('button', 'New link').tap();
    await dialog(fx, 'New link').getByLabel('Program').selectOption(program.name);
    await dialog(fx, 'New link').getByLabel('Code').fill(code);
    await dialog(fx, 'New link').getByLabel('Landing URL').fill(`${process.env.E2E_APP_URL}/register`);
    await dialog(fx, 'New link').getByRole('button', 'Create').tap();
    await expect(screen.getByRole('row').filter({ hasText: code })).toHaveCount(1);

    // The tracker answers /r/<code> with a 302 to the landing page carrying the click id.
    await browser.goto(`/r/${code}`);
    await expect(browser).toHaveURL(/\/register\/?\?bf_click=[0-9a-f-]{36}$/);
  });

  test('the payout request form checks the amount; the server checks it against the balance', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
    const { program } = await w();
    await app.open('/me/payouts');
    await expect(screen.getByRole('heading', 'Payouts').first()).toBeVisible();
    await screen.getByRole('button', 'Request payout').tap();
    await dialog(fx, 'Request a payout').getByLabel('Program').selectOption(program.name);
    // Nothing was earned yet, so nothing can be requested: the API says so.
    await dialog(fx, 'Request a payout').getByLabel('Amount').fill('5000');
    await dialog(fx, 'Request a payout').getByRole('button', 'Send request').tap();
    await expect(dialog(fx, 'Request a payout').getByRole('alert')).toContainText(/payable balance|minimum/);
    // A malformed amount is refused before any request is made.
    await dialog(fx, 'Request a payout').getByLabel('Amount').fill('12.345');
    await dialog(fx, 'Request a payout').getByRole('button', 'Send request').tap();
    await expect(dialog(fx, 'Request a payout').getByRole('alert')).toContainText('amount like');
  });

  test('an accepted request appears in the affiliate request list', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
    const { program } = await w();
    const made = { id: '00000000-0000-4000-8000-0000000000d1', affiliate_id: '00000000-0000-4000-8000-0000000000d2', program_id: program.id, requested_cents: 2550, currency: 'USD', status: 'pending', rejection_reason: null, payout_id: null, created_at: '2026-01-01T00:00:00Z', decided_at: null };
    let posted = '';
    await browser.route(/\/api\/v1\/me\/payout-requests(\?.*)?$/, (route) => {
      if (route.request.method === 'POST') {
        posted = route.request.postData ?? '';
        return route.fulfill({ status: 201, json: made });
      }
      return route.fulfill({ json: posted ? { items: [made], total: 1 } : { items: [], total: 0 } });
    });
    await app.open('/me/payouts');
    await screen.getByRole('button', 'Request payout').tap();
    await dialog(fx, 'Request a payout').getByLabel('Program').selectOption(program.name);
    await dialog(fx, 'Request a payout').getByLabel('Amount').fill('25.50');
    await dialog(fx, 'Request a payout').getByRole('button', 'Send request').tap();
    await expect(dialog(fx, 'Request a payout')).toBeHidden();
    expect(JSON.parse(posted)).toMatchObject({ requested_cents: 2550, currency: 'USD' });
    await expect(screen.getByRole('row').filter({ hasText: '25.50 USD' })).toContainText('pending', { ignoreCase: true });
    await browser.unrouteAll();
  });
});
