import { describe, test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { readFile } from 'node:fs/promises';
import {
  APP, Api, click, confirm, dialog, listUrl as url, openRow, postEvent, sale, section, seedAffiliate, seedWorld, signInAdmin,
  signInAffiliate, signOut, signedHeaders, unique, waitForRow, type Fx, type World,
} from '../fixtures/seed.js';

// Money: conversion ingest, approval, batches and rails, webhook tester, payout requests (admin side).
const SUFFIX = unique();
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

describe('money tools', { serial: true }, () => {
  test('sign in', async (f) => {
    await signInAdmin(f);
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

describe('click to cash', { serial: true }, () => {
  let world: Promise<World> | undefined;
  const w = () => (world ??= seedWorld());
  const saleId = `sale-${unique()}`;

  test('a signed conversion is ingested, a badly signed one is refused', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
    const { program, code } = await w();
    const clickId = await click(code);

    const forged = JSON.stringify({ customer: 'cust-bad', amount_cents: 5000, currency: 'USD', click: clickId });
    const bad = await fetch(`${APP}/v1/events/${program.publicKey}`, {
      method: 'POST',
      headers: signedHeaders('not-the-secret', `bad-${saleId}`, forged),
      body: forged,
    });
    expect(bad.status).toBe(400);

    // Only the click id ties the sale to the affiliate: no referral code in the payload.
    const body = JSON.stringify({ customer: 'cust-1', amount_cents: 5000, currency: 'USD', click: clickId });
    expect((await postEvent(program.publicKey, program.webhookSecret, saleId, body)).status).toBe(202);

    await signInAdmin(fx, (await w()).ws.admin);
    await app.open('/conversions');
    await waitForRow(fx, saleId);
    const row = screen.getByRole('row').filter({ hasText: saleId });
    await expect(row).toContainText((await w()).affiliate.email);
    await expect(row).toContainText('50.00 USD');
    await expect(row).toContainText('10.00 USD');
    await expect(row).toContainText('pending', { ignoreCase: true });
    // The refused webhook left nothing behind.
    await expect(screen.getByText(`bad-${saleId}`)).toHaveCount(0);
  });

  test('admin approves the conversion and pays it out', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
    const { program, affiliate } = await w();
    await openRow(fx, '/conversions', 'Conversions', saleId);
    await confirm(fx, 'Approve', `Approve conversion ${saleId}?`, 'Approve');
    await expect(section(fx, 'Sale')).toContainText('approved', { ignoreCase: true });

    await app.open('/payouts');
    await screen.getByRole('button', 'New batch').tap();
    await dialog(fx, 'New payout batch').getByLabel('Program').selectOption(program.name);
    await dialog(fx, 'New payout batch').getByRole('button', 'Create draft').tap();
    await expect(dialog(fx, /^Batch /)).toBeVisible();
    await expect(section(fx, 'Batch')).toContainText('draft', { ignoreCase: true });
    await expect(section(fx, 'Payouts')).toContainText(affiliate.payout);

    await confirm(fx, 'Approve', 'Approve?', 'Approve');
    await expect(section(fx, 'Batch')).toContainText('approved', { ignoreCase: true });
    await confirm(fx, 'Mark paid', 'Mark paid?', 'Mark paid');
    await expect(section(fx, 'Batch')).toContainText('paid', { ignoreCase: true });

    const file = await browser.waitForDownload(() => screen.getByRole('button', 'Download CSV').tap());
    const csv = await readFile(file.absolutePath, 'utf8');
    expect(csv).toBe(`email,amount,currency\n${affiliate.payout},10.00,USD\n`);
  });

  test('the affiliate sees the paid commission in their earnings', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
    await signOut(fx);
    await signInAffiliate(fx, (await w()).affiliate);
    await app.open('/me/earnings');
    const row = screen.getByRole('row').filter({ hasText: '50.00 USD' });
    await expect(row).toHaveCount(1);
    await expect(row).toContainText('10.00 USD');
    await expect(row).toContainText('paid', { ignoreCase: true });
  });
});

/** A new enrolled affiliate in `w`'s program with 10.00 USD payable and one pending 5.00 request. */
async function withRequest(w: World, tag: string) {
  const affiliate = await seedAffiliate(w.admin, w.ws.slug, w.program.id, `${w.suffix}-${tag}`);
  const aff = await new Api().login(affiliate);
  const code = `r-${w.suffix}-${tag}`;
  await aff.ok('POST', '/api/v1/me/referral-codes', { program_id: w.program.id, code, landing_url: `${APP}/register` });
  await sale({ ...w, code }, await click(code), `req-${w.suffix}-${tag}`);
  const pending = async () => (await w.admin.ok('GET', `/api/v1/conversions?program_id=${w.program.id}&status=pending`)).items;
  await expect.poll(async () => (await pending()).length, { timeout: 45_000, interval: 1_000 }).toBe(1);
  await w.admin.ok('POST', `/api/v1/conversions/${(await pending())[0].id}/approve`);
  const made = await aff.ok('POST', '/api/v1/me/payout-requests', { program_id: w.program.id, requested_cents: 500, currency: 'USD' });
  return { affiliate, requestId: made.id as string };
}

describe('payout requests (admin)', { serial: true }, () => {
  let world: Promise<World> | undefined;
  const w = () => (world ??= seedWorld());

  test('admin sees a pending request and approves it into a draft batch', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
    const world = await w();
    const { affiliate, requestId } = await withRequest(world, 'ok');
    await signInAdmin(fx, world.ws.admin);
    await app.open('/payouts');
    const row = screen.getByRole('row').filter({ hasText: affiliate.email });
    await expect(row).toContainText('5.00 USD');
    await expect(row).toContainText('pending', { ignoreCase: true });
    await row.getByRole('button', `Approve request from ${affiliate.email}`).tap();
    await expect(dialog(fx, 'Approve payout request?')).toContainText('current payable balance');
    await dialog(fx, 'Approve payout request?').getByRole('button', 'Approve').tap();
    await expect(dialog(fx, 'Approve payout request?')).toBeHidden();
    await expect(row).toContainText('approved', { ignoreCase: true });
    // Decided: no actions left, and the server holds the decision.
    await expect(row.getByRole('button', /request from/)).toHaveCount(0);
    expect((await world.admin.ok('GET', '/api/v1/payout-requests?status=approved')).items.map((i: { id: string }) => i.id)).toContain(requestId);
  });

  test('admin rejects with a reason; it is required and the affiliate sees it', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
    const world = await w();
    const { affiliate } = await withRequest(world, 'no');
    await app.open('/payouts');
    await browser.reload();
    const row = screen.getByRole('row').filter({ hasText: affiliate.email });
    await row.getByRole('button', `Reject request from ${affiliate.email}`).tap();
    // An empty reason never leaves the browser.
    await dialog(fx, 'Reject payout request').getByRole('button', 'Reject').tap();
    await expect(dialog(fx, 'Reject payout request')).toBeVisible();
    await dialog(fx, 'Reject payout request').getByLabel('Reason').fill('Tax form missing');
    await dialog(fx, 'Reject payout request').getByRole('button', 'Reject').tap();
    await expect(dialog(fx, 'Reject payout request')).toBeHidden();
    await expect(row).toContainText('rejected', { ignoreCase: true });
    await expect(row).toContainText('Tax form missing');

    await signOut(fx);
    await signInAffiliate(fx, affiliate);
    await app.open('/me/payouts');
    await expect(screen.getByRole('row').filter({ hasText: 'Tax form missing' })).toContainText('rejected', { ignoreCase: true });
  });

  test('a request the server already decided shows the conflict', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
    const world = await w();
    const { affiliate, requestId } = await withRequest(world, 'race');
    await signOut(fx);
    await signInAdmin(fx, world.ws.admin);
    await app.open('/payouts');
    const row = screen.getByRole('row').filter({ hasText: affiliate.email });
    await expect(row).toContainText('pending', { ignoreCase: true });
    // Decided behind the page's back (another admin): the stale button reports it, never double-decides.
    await world.admin.ok('POST', `/api/v1/payout-requests/${requestId}/reject`, { reason: 'other admin' });
    await row.getByRole('button', `Approve request from ${affiliate.email}`).tap();
    await dialog(fx, 'Approve payout request?').getByRole('button', 'Approve').tap();
    await expect(dialog(fx, 'Approve payout request?').getByRole('alert')).toContainText('already decided');
  });
});
