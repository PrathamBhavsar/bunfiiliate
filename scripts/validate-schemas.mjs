// Validates schemas/tracker/v2: every schema compiles, *.valid.json pass, *.invalid.json fail
// with the keyword named in the filename, and every contract rule has an invalid example.
// Run via `make schemas-check` (installs ajv into scripts/schemas first).
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "schemas", "tracker", "v2");
const require = createRequire(join(dirname(fileURLToPath(import.meta.url)), "schemas", "x.js"));
const Ajv2020 = require("ajv/dist/2020").default;
const addFormats = require("ajv-formats");

const ajv = new Ajv2020({ allErrors: true, strict: true, strictRequired: false });
addFormats(ajv);

const names = readdirSync(root).filter((f) => f.endsWith(".schema.json")).map((f) => f.replace(".schema.json", ""));
const validators = {};
for (const n of names) validators[n] = ajv.compile(JSON.parse(readFileSync(join(root, `${n}.schema.json`), "utf8")));

// JSON.parse turns 49.0 into 49, so integer-ness of amount_cents is also checked lexically
// on the raw text. The server MUST do the same (reject any '.', 'e' or 'E' in the token).
const lexicalAmountBad = (raw) => /"amount_cents"\s*:\s*-?\d+(\.\d+|[eE])/.test(raw);

const fails = [];
const invalidFiles = [];
const dir = join(root, "examples");
for (const f of readdirSync(dir).filter((x) => x.endsWith(".json")).sort()) {
  const parts = f.replace(/\.json$/, "").split(".");
  const schema = parts[0];
  const kind = parts[parts.length - 1];
  if (!validators[schema]) { fails.push(`${f}: unknown schema '${schema}'`); continue; }
  const raw = readFileSync(join(dir, f), "utf8");
  const data = JSON.parse(raw);
  const ok = validators[schema](data) && !(schema === "event" && lexicalAmountBad(raw));
  if (kind === "valid") {
    if (!ok) fails.push(`${f}: expected valid, got ${JSON.stringify(validators[schema].errors)}`);
  } else if (kind === "invalid") {
    invalidFiles.push(f);
    const keyword = parts[parts.length - 2];
    if (ok) { fails.push(`${f}: expected invalid, passed`); continue; }
    const kws = (validators[schema].errors ?? []).map((e) => e.keyword);
    if (keyword === "lexical") {
      if (!(schema === "event" && lexicalAmountBad(raw))) fails.push(`${f}: lexical rule did not trigger`);
    } else if (!kws.includes(keyword)) fails.push(`${f}: expected keyword '${keyword}', got [${kws}]`);
  } else fails.push(`${f}: name must end .valid.json or .invalid.json`);
}

// Every normative rule in docs/tracker-contract-v2.md has at least one invalid example.
const rules = {
  "beacon v=2": /^click-beacon\.version/,
  "beacon code|click": /^click-beacon\.no-code/,
  "beacon vid length/charset": /^click-beacon\.(short|long|)-?vid/,
  "beacon url http(s) <=2048": /^click-beacon\.(ftp-url|long-url)/,
  "beacon ref <=2048": /^click-beacon\.long-ref/,
  "beacon no NUL": /^click-beacon\.nul/,
  "event amount integer": /^event\.float-amount/,
  "event amount lexical (49.0)": /^event\.integral-float-amount/,
  "event amount >=0": /^event\.negative-amount/,
  "event amount required for sale": /^event\.sale-missing-amount/,
  "event currency ISO uppercase": /^event\.lowercase-currency/,
  "event external_id 1..128": /^event\.(empty|long)-external-id/,
  "event ref_external_id for refund": /^event\.refund-missing-ref/,
  "event meta <=20 keys": /^event\.too-many-meta/,
  "event meta scalar values": /^event\.nested-meta/,
  "event no NUL": /^event\.nul-/,
  "event deny unknown fields": /^event\.unknown-field/,
  "event never raw email": /^event\.raw-email/,
  "error code snake_case": /^error\.camelcase/,
};
for (const [rule, re] of Object.entries(rules)) {
  if (!invalidFiles.some((f) => re.test(f))) fails.push(`rule without invalid example: ${rule}`);
}

if (fails.length) { console.error(fails.join("\n")); process.exit(1); }
console.log(`schemas-check ok: ${names.length} schemas, ${readdirSync(dir).length} examples, ${Object.keys(rules).length} rules`);
