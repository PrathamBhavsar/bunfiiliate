import { QueryClient } from "@tanstack/react-query";

export const queryClient = new QueryClient({
  defaultOptions: { queries: { staleTime: 30_000, retry: false, refetchOnWindowFocus: false } },
});

/** Drop every cached query except the session, so one user's data never shows for the next. */
export function clearUserData(sessionKey: readonly unknown[]) {
  queryClient.removeQueries({ predicate: (q) => q.queryKey[0] !== sessionKey[0] });
}
