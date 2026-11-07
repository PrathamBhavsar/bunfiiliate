import { describe, test } from '@e2e-dev/web';
import { expect } from 'e2e';

const env = (name: string) => {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is not set; run this suite through scripts/e2e.sh`);
  return value;
};

const ADMIN = { email: env('E2E_ADMIN_EMAIL'), password: env('E2E_ADMIN_PASSWORD') };

describe('auth pages', () => {
  test('settings security section starts TOTP enrolment without confirming it', async ({ app, screen, browser }) => {
    await app.open('/login');
    await screen.getByLabel('Email').fill(ADMIN.email);
    await screen.getByLabel('Password').fill(ADMIN.password);
    await screen.getByRole('button', 'Sign in').tap();
    await expect(browser).toHaveURL(/\/dashboard\/?$/);

    await app.open('/settings');
    await expect(screen.getByRole('heading', 'Security')).toBeVisible();
    await screen.getByRole('button', 'Set up').tap();
    // Left unconfirmed: the admin keeps password-only login for the other suites.
    await expect(browser.getByTestId('totp-secret')).toBeVisible();
  });

  test('reset page rejects a bad token', async ({ app, screen }) => {
    await app.open('/reset-password');
    await screen.getByLabel('Reset token').fill('not-a-real-token');
    await screen.getByLabel('New password (12+ characters)').fill('a-long-new-password');
    await screen.getByRole('button', 'Reset password').tap();
    await expect(screen.getByRole('alert')).toBeVisible();
  });
});
