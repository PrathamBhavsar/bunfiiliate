import { statSync } from "node:fs";

const file = new URL("../crates/tracker/js/tracker.min.js", import.meta.url);
const limit = 2048;
const size = statSync(file).size;
console.log(`tracker.min.js: ${size} bytes (limit < ${limit})`);
if (size >= limit) process.exit(1);
