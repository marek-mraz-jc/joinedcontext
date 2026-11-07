import { useSyncExternalStore } from "react";

/**
 * The session ended under an open page (UI-16, T-2747): the login address to go to, or null.
 *
 * A 401 used to send the browser to the login page at once, which threw away whatever the
 * person had typed on the page. It now raises this instead, and the dialog it opens lets them
 * sign in in another tab and carry on where they were.
 */
let ended: string | null = null;
const listeners = new Set<() => void>();

function emit(): void {
  for (const listener of listeners) listener();
}

/**
 * Nobody listening means nobody is signed in (the dialog is mounted for a signed-in person
 * only): an anonymous visitor's 401 is the router's way to the login page, not an ended session.
 */
export function announceSessionEnded(loginUrl: string): void {
  if (listeners.size === 0 || ended === loginUrl) return;
  ended = loginUrl;
  emit();
}

export function clearSessionEnded(): void {
  if (ended === null) return;
  ended = null;
  emit();
}

/** Stable, so React subscribes once per mount and an unsubscribe means the dialog is gone. */
function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) ended = null;
  };
}

function snapshot(): string | null {
  return ended;
}

export function useSessionEnded(): string | null {
  return useSyncExternalStore(subscribe, snapshot);
}
