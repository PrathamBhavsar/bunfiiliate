import type { E2EConfig } from 'e2e';
import { web } from '@e2e-dev/web';

// Deterministic steps only (locators + expect): no model, no API key.
export default {
  targets: [{ engine: web(), app: { url: process.env.E2E_APP_URL ?? 'http://localhost:18000' } }],
  output: '.reports',
  timeout: 240_000,
  workers: 1,
  reporters: ['list', 'junit'],
} satisfies E2EConfig;
