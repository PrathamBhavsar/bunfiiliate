// coverage_table_lists_every_page: docs/ui/e2e-plan.md must list every page under apps/web/src/app
// with a test file that exists and contains the named test title.
import assert from 'node:assert/strict';
import { readdirSync, readFileSync, existsSync, statSync } from 'node:fs';
import { join, relative, sep, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const appDir = join(root, 'apps/web/src/app');

function pages(dir) {
  return readdirSync(dir).flatMap((name) => {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) return pages(full);
    return name === 'page.tsx' ? [full] : [];
  });
}

const routeOf = (file) => {
  const parts = relative(appDir, dirname(file)).split(sep).filter((p) => p && !/^\(.*\)$/.test(p));
  return '/' + parts.join('/');
};

const rows = readFileSync(join(root, 'docs/ui/e2e-plan.md'), 'utf8')
  .split('\n')
  .filter((l) => /^\| `\//.test(l))
  .map((l) => l.split('|').map((c) => c.trim()).filter(Boolean));

test('coverage_table_lists_every_page', () => {
  const actual = pages(appDir).map(routeOf).sort();
  const listed = rows.map((r) => r[0].replaceAll('`', '')).sort();
  assert.deepEqual(listed, actual);
});

test('coverage_table_tests_exist', () => {
  for (const [page, file, title] of rows) {
    const path = join(root, 'e2e/tests', file.replaceAll('`', ''));
    assert.ok(existsSync(path), `${page}: missing ${file}`);
    const src = readFileSync(path, 'utf8');
    assert.ok(src.includes(title.replaceAll('`', '')), `${page}: "${title}" not in ${file}`);
  }
});
