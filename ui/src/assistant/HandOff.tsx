import { Fragment, useState, useSyncExternalStore } from "react";
import type { JSX, ReactNode } from "react";
import { useRouter, useRouterState } from "@tanstack/react-router";
import { handOffCount, subscribeHandOffs } from "./state";

/** What the assistant leaves in the address for the page it opens (AG-73, AG-77). */
export const HAND_OFF = ["edit", "delete", "grant", "draft", "space", "endpoint"] as const;

/**
 * Its page, mounted afresh for each hand-off in the address. A page takes what the assistant
 * handed it once, when it mounts, so a person already on that page would otherwise see nothing
 * open when the assistant sends them there again.
 */
/** The query a page last wrote for itself: the same keys, and no hand-off (T-3239). */
let own: string | null = null;

/**
 * Puts a page's own choices in the address in place of its current query (UI-89): a reload or a
 * sent link opens them again. The page wrote it, so it is no hand-off and `HandOff` does not
 * mount the page afresh for it.
 */
export function replaceOwnSearch(query: URLSearchParams): void {
  const { pathname, search, hash } = window.location;
  const next = query.toString();
  if (next === new URLSearchParams(search).toString()) return;
  own = next;
  window.history.replaceState(window.history.state, "", `${pathname}${next ? `?${next}` : ""}${hash}`);
}

export function HandOff({ children }: { children: ReactNode }): JSX.Element {
  const history = useRouter().history;
  const searchStr = useRouterState({ select: (state) => state.location.searchStr.replace(/^\?/, "") });
  const inAddress = useRouterState({
    select: (state) => {
      const search = new URLSearchParams(state.location.searchStr);
      return HAND_OFF.map((name) => search.get(name) ?? "").join("\n");
    },
  });
  // A prefill handed again at the same address is a hand-off too (JC2-11).
  const handed = `${inAddress}\n${useSyncExternalStore(subscribeHandOffs, handOffCount)}`;
  // The router moves before the address does (its history writes the browser's a microtask
  // later), and the page reads its hand-off from the address as it mounts (T-0770).
  history.flush();
  // An address the page wrote for itself keeps the page mounted; any other hand-off mounts it
  // afresh (React's state derived during render, no effect and no second paint).
  const [key, setKey] = useState(handed);
  if (handed !== key && new URLSearchParams(searchStr).toString() !== own) {
    setKey(handed);
  }
  return <Fragment key={key}>{children}</Fragment>;
}
