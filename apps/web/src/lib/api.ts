import { type components, createApi } from "@bunfiiliate/sdk";
import { clearUserData, queryClient } from "./query";

export type User = components["schemas"]["UserView"];
export const meKey = ["me"] as const;

const AUTH_PREFIX = "/api/v1/auth/";

let refreshing: Promise<boolean> | null = null;

// One refresh at a time: concurrent 401s share it, and the refresh token rotates on every use.
function refreshSession() {
  refreshing ??= api
    .POST("/api/v1/auth/refresh")
    .then(({ data }) => {
      if (data) queryClient.setQueryData(meKey, data);
      return data !== undefined;
    })
    .finally(() => {
      refreshing = null;
    });
  return refreshing;
}

const CSRF_COOKIE = "bf_csrf";
const SAFE_METHODS = new Set(["GET", "HEAD", "OPTIONS"]);

function csrfCookie(): string | undefined {
  return document.cookie
    .split("; ")
    .find((c) => c.startsWith(`${CSRF_COOKIE}=`))
    ?.slice(CSRF_COOKIE.length + 1);
}

// Double-submit CSRF: echo the readable bf_csrf cookie as X-CSRF-Token on every mutation.
// Without a cookie yet (first login), GET /auth/csrf sets one.
async function withCsrf(request: Request): Promise<Request> {
  if (SAFE_METHODS.has(request.method)) return request;
  let token = csrfCookie();
  if (!token) {
    const res = await fetch("/api/v1/auth/csrf", { credentials: "include" });
    token = ((await res.json()) as { token: string }).token;
  }
  const next = new Request(request);
  next.headers.set("X-CSRF-Token", token);
  return next;
}

// On a 401 from a non-auth call, refresh once and replay the request. A failed refresh ends the session.
async function fetchWithRefresh(input: Request) {
  const request = await withCsrf(input);
  const replay = request.clone();
  const response = await fetch(request);
  if (response.status !== 401 || new URL(request.url).pathname.startsWith(AUTH_PREFIX)) {
    return response;
  }
  if (await refreshSession()) return fetch(await withCsrf(replay));
  clearUserData(meKey);
  queryClient.setQueryData(meKey, null);
  return response;
}

export const api = createApi({ fetch: fetchWithRefresh });

export function errorMessage(error: unknown): string {
  if (error instanceof TypeError) return "Cannot reach the server.";
  if (error instanceof Error) return error.message;
  const body = error as components["schemas"]["ErrorBody"] | undefined;
  return body?.error?.message ?? "Something went wrong.";
}
