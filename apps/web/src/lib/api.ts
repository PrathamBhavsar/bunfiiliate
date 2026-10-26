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

// On a 401 from a non-auth call, refresh once and replay the request. A failed refresh ends the session.
async function fetchWithRefresh(request: Request) {
  const replay = request.clone();
  const response = await fetch(request);
  if (response.status !== 401 || new URL(request.url).pathname.startsWith(AUTH_PREFIX)) {
    return response;
  }
  if (await refreshSession()) return fetch(replay);
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
