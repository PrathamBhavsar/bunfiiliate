import { statSync } from "node:fs";

const file = new URL("../crates/tracker/js/tracker.min.js", import.meta.url);
const limit = 2048;
const size = statSync(file).size;
console.log(`tracker.min.js: ${size} bytes (limit < ${limit})`);
if (size >= limit) process.exit(1);

// v2 script: budget raised to 2560 B (v2 adds vid storage fallback, pk, DNT, validation).
const v2 = statSync(new URL("../crates/tracker/js/tracker.v2.min.js", import.meta.url)).size;
console.log(`tracker.v2.min.js: ${v2} bytes (limit < 2560)`);
if (v2 >= 2560) process.exit(1);
