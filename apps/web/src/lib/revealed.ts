/**
 * Secrets the API returns exactly once (on create or rotate). Held in memory for this tab only:
 * a reload forgets them, and the API never returns them again.
 */
const store = new Map<string, string>();

export const remember = (kind: string, id: string, secret: string | null | undefined) => {
  if (secret) store.set(`${kind}:${id}`, secret);
};

export const forget = (kind: string, id: string) => store.delete(`${kind}:${id}`);

export const recall =(kind: string, id: string): string | undefined => store.get(`${kind}:${id}`);
