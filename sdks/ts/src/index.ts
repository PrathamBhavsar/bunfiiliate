import createClient, { type ClientOptions } from "openapi-fetch";
import type { paths } from "./schema";

export type { components, paths } from "./schema";

/** Typed same-origin API client; cookies ride along on every call. */
export function createApi(options: ClientOptions = {}) {
  return createClient<paths>({ baseUrl: "", credentials: "include", ...options });
}

export type Api = ReturnType<typeof createApi>;
