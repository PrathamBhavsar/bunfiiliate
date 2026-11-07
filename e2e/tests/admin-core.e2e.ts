import { describe, test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { ADMIN, WORKSPACE, openRow, confirm, dialog, section, signIn, signInAdmin, signOut, unique, type Fx } from '../fixtures/seed.js';

// Admin shell: auth pages, billing and channels, program setup, affiliate approval, page smoke.
const SUFFIX = unique();
const PROGRAM = `Core ${SUFFIX}`;
const AFFILIATE = { email: `core-${SUFFIX}@e2e.test`, password: 'e2e-affiliate-pw', payout: `core-pay-${SUFFIX}@e2e.test` };

/** Every admin page: URL and its `<h1>`. */
const ADMIN_PAGES: [string, string][] = [
  ['/dashboard', 'Dashboard'],
  ['/programs', 'Programs'],
  ['/affiliates', 'Affiliates'],
  ['/referral-codes', 'Referral codes'],
  ['/conversions', 'Conversions'],
  ['/customers', 'Customers'],
  ['/ledger', 'Ledger'],
  ['/payouts', 'Payouts'],
  ['/analytics', 'Analytics'],
  ['/reports', 'Reports'],
  ['/webhooks', 'Webhooks'],
  ['/webhooks/test', 'Webhook tester'],
  ['/notifications', 'Notifications'],
  ['/audit-log', 'Audit log'],
  ['/contact-requests', 'Contact requests'],
  ['/email-templates', 'Email templates'],
  ['/settings', 'Settings'],
  ['/settings/billing', 'Billing'],
];

describe('admin core', { serial: true }, () => {
  test('sign in', async (f) => {
    await signInAdmin(f);
  });

  test('settings security section starts TOTP enrolment without confirming it', async ({ app, screen, browser }) => {
    await app.open('/settings');
    await expect(screen.getByRole('heading', 'Security')).toBeVisible();
    await screen.getByRole('button', 'Set up').tap();
    // Left unconfirmed: the admin keeps password-only login for the other suites.
    await expect(browser.locator('[data-testid="totp-secret"]')).toBeVisible();
  });

  test('billing page shows the plan and usage; settings stores a channel URL write-only', async ({ app, screen, browser }) => {
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

  test('every admin page renders its heading', async ({ app, screen, browser }) => {
    for (const [path, title] of ADMIN_PAGES) {
      await app.open(path);
      await expect(screen.getByRole('heading', title).first()).toBeVisible();
      await expect(browser.locator('main')).toBeVisible();
    }
  });

  test('admin creates a program with a site and a commission plan', async ({ app, screen, browser }) => {
    const fx = { app, screen, browser };
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
    const key = ((await browser.locator('dt:text-is("Public key") + dd').textContent()) ?? '').trim();
    const secret = ((await browser.locator('dt:text-is("Webhook secret") + dd').textContent()) ?? '').trim();
    expect(key).toMatch(/^\S+$/);
    expect(secret).not.toContain('•');
    expect(secret.length).toBeGreaterThan(8);
  });

  test('a visitor registers as an affiliate and the admin approves and enrolls them', async ({ app, screen, browser }) => {
    const fx: Fx = { app, screen, browser };
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

  test('reset page rejects a bad token', async ({ app, screen }) => {
    await app.open('/reset-password');
    await screen.getByLabel('Reset token').fill('not-a-real-token');
    await screen.getByLabel('New password (12+ characters)').fill('a-long-new-password');
    await screen.getByRole('button', 'Reset password').tap();
    await expect(screen.getByRole('alert')).toBeVisible();
  });
});
