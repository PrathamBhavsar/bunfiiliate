import { describe, test } from '@e2e-dev/web';
import { expect } from 'e2e';

const env = (name: string) => {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is not set; run this suite through scripts/e2e.sh`);
  return value;
};

const ADMIN = { email: env('E2E_ADMIN_EMAIL'), password: env('E2E_ADMIN_PASSWORD') };

describe('billing and notify channels', () => {
  test('billing page shows the plan and usage; settings stores a channel URL write-only', async ({ app, screen, browser }) => {
    await app.open('/login');
    await screen.getByLabel('Email').fill(ADMIN.email);
    await screen.getByLabel('Password').fill(ADMIN.password);
    await screen.getByRole('button', 'Sign in').tap();
    await expect(browser).toHaveURL(/\/dashboard\/?$/);

    // The seeded default plan is Free; the e2e stack has no other plan, so no upgrade button.
    await app.open('/settings/billing');
    await expect(screen.getByRole('heading', 'Billing')).toBeVisible();
    await expect(browser.locator('[data-testid="current-plan"]')).toHaveText('Free');
    await expect(browser.locator('[data-testid="usage-Programs"]')).toBeVisible();
    await expect(browser.locator('[data-testid="usage-Affiliates"]')).toBeVisible();

    // A literal public IP keeps the SSRF check off DNS. Nothing is sent: no test message here.
    await app.open('/settings');
    await screen.getByLabel('Discord webhook URL').fill('https://8.8.8.8/webhook/e2e-token');
    await screen.getByRole('button', 'Save channels').tap();
    await expect(screen.getByRole('status')).toBeVisible();
    await expect(screen.getByText('(set)')).toBeVisible();
    // The URL is never read back into the page.
    await expect(browser.locator('#notify-discord')).toHaveValue('');
  });
});
