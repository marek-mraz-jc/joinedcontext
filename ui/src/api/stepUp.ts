import { useSyncExternalStore } from "react";
import type { Middleware } from "openapi-fetch";

/**
 * A decision door answered `403 second-factor-required` (T-5567): the step-up login to go to,
 * or null. The right came from a role binding, and the person signed in with a password alone;
 * a login with the one-time code lets the same decision through. Like the ended session
 * (sessionEnded.ts), the page stays as it is while the person steps up in another tab.
 */
let pending: string | null = null;
const listeners = new Set<() => void>();

function emit(): void {
  for (const listener of listeners) listener();
}

export function announceStepUp(loginUrl: string): void {
  if (listeners.size === 0 || pending === loginUrl) return;
  pending = loginUrl;
  emit();
}

export function clearStepUp(): void {
  if (pending === null) return;
  pending = null;
  emit();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) pending = null;
  };
}

export function useStepUp(): string | null {
  return useSyncExternalStore(subscribe, () => pending);
}

/** The Portal's own login asking the realm for the one-time code, back to `pathAndSearch`. */
export function stepUpUrl(pathAndSearch: string): string {
  return `/api/v1/auth/login?step_up=true&redirect_to=${encodeURIComponent(pathAndSearch)}`;
}

const SECOND_FACTOR = "https://joinedcontext.com/errors/second-factor-required";

/** Raises the step-up on the decision door's refusal; the response goes on to the page untouched. */
export function createStepUpMiddleware(raise: (loginUrl: string) => void): Middleware {
  return {
    async onResponse({ response }) {
      if (response.status !== 403) return response;
      const problem: unknown = await response
        .clone()
        .json()
        .catch(() => null);
      if (typeof problem === "object" && problem !== null && (problem as { type?: unknown }).type === SECOND_FACTOR) {
        raise(stepUpUrl(`${window.location.pathname}${window.location.search}`));
      }
      return response;
    },
  };
}
