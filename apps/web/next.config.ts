import { fileURLToPath } from "node:url";
import type { NextConfig } from "next";

const config: NextConfig = {
  output: "export",
  images: { unoptimized: true },
  // The SDK lives in the pnpm workspace outside this dir and ships TypeScript source.
  transpilePackages: ["@bunfiiliate/sdk"],
  turbopack: { root: fileURLToPath(new URL("../..", import.meta.url)) },
  agentRules: false,
};

export default config;
