// Reads {"schema":"event|click-beacon|error","bodies":[raw,...]} on stdin, prints a JSON array of
// booleans: does each raw body pass the v2 JSON Schema (plus the lexical amount rule)?
// Used by crates/tracker/tests/contract.rs; needs `npm --prefix scripts/schemas install`.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(join(here, "schemas", "x.js"));
const Ajv2020 = require("ajv/dist/2020").default;
const addFormats = require("ajv-formats");
const ajv = new Ajv2020({ allErrors: true, strict: true, strictRequired: false });
addFormats(ajv);
const root = join(here, "..", "schemas", "tracker", "v2");
const lexicalAmountBad = (raw) => /"amount_cents"\s*:\s*-?\d+(\.\d+|[eE])/.test(raw);

const req = JSON.parse(readFileSync(0, "utf8"));
const validate = ajv.compile(JSON.parse(readFileSync(join(root, `${req.schema}.schema.json`), "utf8")));
const out = req.bodies.map((raw) => {
  let data;
  try { data = JSON.parse(raw); } catch { return false; }
  return validate(data) && !(req.schema === "event" && lexicalAmountBad(raw));
});
process.stdout.write(JSON.stringify(out));
