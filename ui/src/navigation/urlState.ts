import { useCallback } from "react";
import { useRouter, useRouterState } from "@tanstack/react-router";

/**
 * One choice of a page kept in the address's query (UI-89, T-3239): a tab, a view. A new choice
 * is a new history entry, so back and forward walk the choices and a reload or a copied link
 * opens the same one. The default is no parameter at all, and a value the page does not know
 * reads as the default. The rest of the query, the language override included, is kept.
 */
export function useUrlParam<T extends string>(key: string, fallback: T, allowed: readonly T[]): [T, (next: T) => void] {
  const router = useRouter();
  const raw = useRouterState({ select: (state) => new URLSearchParams(state.location.searchStr).get(key) });
  const value = raw !== null && (allowed as readonly string[]).includes(raw) ? (raw as T) : fallback;
  const set = useCallback(
    (next: T) => {
      const { pathname, search, hash } = router.history.location;
      const query = new URLSearchParams(search);
      if (next === fallback) query.delete(key);
      else query.set(key, next);
      router.history.push(addressOf(pathname, query, hash));
    },
    [router, key, fallback],
  );
  return [value, set];
}

/** The longest entity id the address carries; NGSI-LD names no limit, a URL does. */
const MAX_URN = 1024;

/** Marks the history entry an open pushed, so the close knows Back reaches the address before it. */
const OPENED = "jcOpened";

/**
 * One entity named in the address's query (`?row=<urn>`, T-4828), so a link opens that row. Only an
 * NGSI-LD URN up to 1024 characters is read; anything else reads as none. Opening is a new history
 * entry, so back closes it; `null` closes it without one (T-5602): back to the entry before the open
 * when this page pushed it, else in place, so Back after a close leaves the page instead of opening
 * the row again. A close also drops `also`, the keys that only mean something with the row open.
 */
export function useUrlUrn(key: string, also: readonly string[] = []): [string | null, (next: string | null) => void] {
  const router = useRouter();
  const raw = useRouterState({ select: (state) => new URLSearchParams(state.location.searchStr).get(key) });
  const value = raw !== null && raw.startsWith("urn:") && raw.length <= MAX_URN ? raw : null;
  const alsoKeys = also.join(",");
  const set = useCallback(
    (next: string | null) => {
      const { pathname, search, hash, state } = router.history.location;
      const query = new URLSearchParams(search);
      if ((query.get(key) ?? null) === next) return;
      if (next === null && (state as { [OPENED]?: string } | undefined)?.[OPENED] === key) {
        router.history.back();
        return;
      }
      if (next === null) {
        query.delete(key);
        for (const one of alsoKeys.split(",")) if (one) query.delete(one);
        router.history.replace(addressOf(pathname, query, hash));
        return;
      }
      query.set(key, next);
      router.history.push(addressOf(pathname, query, hash), { [OPENED]: key });
    },
    [router, key, alsoKeys],
  );
  return [value, set];
}

/** The longest free text the address carries, such as a search. */
const MAX_TEXT = 200;

/**
 * One free value kept in the address in place, without a history entry (T-5602): a search, a tab or
 * an attribute inside an open row, so a reload or a sent link opens the same one while Back still
 * leaves the page. Empty or `null` removes the key; a value over 200 characters reads as none. What
 * the value names is the page's to check.
 */
export function useUrlText(key: string): [string | null, (next: string | null) => void] {
  const router = useRouter();
  const raw = useRouterState({ select: (state) => new URLSearchParams(state.location.searchStr).get(key) });
  const value = raw !== null && raw !== "" && raw.length <= MAX_TEXT ? raw : null;
  const set = useCallback(
    (next: string | null) => {
      const { pathname, search, hash, state } = router.history.location;
      const query = new URLSearchParams(search);
      const kept = next === null || next === "" ? null : next.slice(0, MAX_TEXT);
      if ((query.get(key) ?? null) === kept) return;
      if (kept === null) query.delete(key);
      else query.set(key, kept);
      // The entry keeps its state, so a close still knows this entry was the row's open.
      router.history.replace(addressOf(pathname, query, hash), state);
    },
    [router, key],
  );
  return [value, set];
}

function addressOf(pathname: string, query: URLSearchParams, hash: string): string {
  const rest = query.toString();
  return `${pathname}${rest ? `?${rest}` : ""}${hash}`;
}
