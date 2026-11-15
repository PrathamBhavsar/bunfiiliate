import { describe, test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { click, sale, seedWorld, signInAdmin, signInAffiliate, signOut, type World } from '../fixtures/seed.js';

// C31 code stats pages, admin and affiliate side, against real tracked clicks and a sale.
let world: Promise<World> | undefined;
const w = () => (world ??= (async () => {
  const s = await seedWorld();
  await click(s.code);
  await sale(s, await click(s.code), `stats-${s.suffix}`);
  return s;
})());

describe('code stats', { serial: true }, () => {
  test('code_stats_page_renders_kpis', async ({ app, screen, browser }) => {
    const { code } = await w();
    await signInAdmin({ app, screen, browser }, (await w()).ws.admin);
    await app.open(`/referral-codes/detail?code=${code}`);
    await expect(browser.locator('#page-title')).toContainText(code);
    const kpis = browser.locator('[data-testid="code-kpis"]');
    await expect(kpis).toBeVisible();
    // Clicks are spooled and ingested asynchronously: wait for both clicks and the sale to land.
    await expect
      .poll(async () => {
        await browser.reload();
        return (await kpis.textContent()) ?? '';
      }, { timeout: 45_000, interval: 1_000 })
      .toMatch(/Clicks\s*2 .*Conversions\s*1 /);
    await expect(kpis).toContainText('Commission');
  });

  test('code_stats_page_unknown_code_shows_not_found_state', async ({ app, browser }) => {
    await w();
    await app.open('/referral-codes/detail?code=no-such-code-xyz');
    await expect(browser.locator('[data-testid="code-not-found"]')).toContainText('Code not found');
    await expect(browser.locator('[data-testid="code-kpis"]')).toHaveCount(0);
  });

  test('the affiliate sees stats for their own code', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
    const s = await w();
    await signOut(fx);
    await signInAffiliate(fx, s.affiliate);
    await app.open(`/me/links/detail?code=${s.code}`);
    await expect(browser.locator('[data-testid="code-kpis"]')).toBeVisible();
    await app.open('/me/links/detail?code=no-such-code-xyz');
    await expect(browser.locator('[data-testid="code-not-found"]')).toBeVisible();
  });
});
