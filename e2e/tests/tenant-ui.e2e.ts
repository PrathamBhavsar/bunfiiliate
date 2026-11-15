import { describe, test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { click, seedAffiliate, seedWorkspace, seedWorld, signInAdmin, signInAffiliate, signOut, type World } from '../fixtures/seed.js';

// C34 tenant checks through the UI: workspace A users never see workspace B's data. Workspace B is a
// second self-serve signup (the e2e stack runs with SIGNUP_ENABLED=1).
type Foreign = { code: string; program: string; affiliateEmail: string };

async function seedForeign(): Promise<Foreign> {
  const { slug, admin } = await seedWorkspace();
  const tag = slug.slice(3);
  const program = await admin.ok('POST', '/api/v1/programs', { name: `B-SECRET-PROGRAM-${tag}`, currency: 'USD' });
  await admin.ok('POST', `/api/v1/programs/${program.id}/sites`, { domain: 'localhost' });
  const aff = await seedAffiliate(admin, slug, program.id, `b-secret-${tag}`);
  const code = `bcode-${tag}`;
  await admin.ok('POST', '/api/v1/referral-codes', { affiliate_id: aff.id, program_id: program.id, code, landing_url: 'http://localhost/' });
  await click(code);
  return { code, program: program.name, affiliateEmail: aff.email };
}

let foreign: Promise<Foreign> | undefined;
let a1: Promise<World> | undefined;
let a2: Promise<World> | undefined;

describe('tenant isolation in the UI', { serial: true }, () => {
  test('ws_a_admin_cannot_open_ws_b_code_page', async ({ app, screen, browser }) => {
    const b = await (foreign ??= seedForeign());
    await signInAdmin({ app, screen, browser }, (await (a1 ??= seedWorld())).ws.admin);
    await app.open(`/referral-codes/detail?code=${b.code}`);
    await expect(browser.locator('[data-testid="code-not-found"]')).toBeVisible();
    await expect(browser.locator('[data-testid="code-kpis"]')).toHaveCount(0);
    const text = (await browser.locator('body').textContent()) ?? '';
    expect(text).not.toContain(b.program);
    expect(text).not.toContain(b.affiliateEmail);
  });

  test('ws_a_admin_lists_never_show_ws_b_rows', async ({ app, browser }) => {
    const b = await (foreign ??= seedForeign());
    // Our own data is there, so an empty list would not prove anything.
    await (a1 ??= seedWorld());
    for (const path of ['/referral-codes', '/affiliates', '/programs']) {
      await app.open(path);
      await expect(browser.locator('tbody tr').first()).toBeVisible();
      const text = (await browser.locator('body').textContent()) ?? '';
      expect(text).not.toContain(b.code);
      expect(text).not.toContain(b.program);
      expect(text).not.toContain(b.affiliateEmail);
    }
  });

  test('affiliate_cannot_view_other_affiliates_code_stats', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
    const mine = await (a1 ??= seedWorld());
    const other = await (a2 ??= seedWorld());
    const b = await (foreign ??= seedForeign());
    await signOut(fx);
    await signInAffiliate(fx, mine.affiliate);
    for (const code of [other.code, b.code]) {
      await app.open(`/me/links/detail?code=${code}`);
      await expect(browser.locator('[data-testid="code-not-found"]')).toBeVisible();
      await expect(browser.locator('[data-testid="code-kpis"]')).toHaveCount(0);
    }
    await app.open(`/me/links/detail?code=${mine.code}`);
    await expect(browser.locator('[data-testid="code-kpis"]')).toBeVisible();
    // Another affiliate's code does not appear in my list either.
    await app.open('/me/links');
    await expect(screen.getByRole('row').filter({ hasText: mine.code })).toHaveCount(1);
    expect(((await browser.locator('body').textContent()) ?? '')).not.toContain(other.code);
  });
});
