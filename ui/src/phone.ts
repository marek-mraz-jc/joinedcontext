import { useSyncExternalStore } from "react";

/** Below Tailwind's `sm`. */
const PHONE = "(max-width: 39.98rem)";

function onPhoneChange(changed: () => void): () => void {
  const query = typeof window.matchMedia === "function" ? window.matchMedia(PHONE) : null;
  query?.addEventListener("change", changed);
  return () => query?.removeEventListener("change", changed);
}

function isPhone(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia(PHONE).matches;
}

/** Whether the page is as narrow as a phone; a component that lays out in script asks this. */
export function useIsPhone(): boolean {
  return useSyncExternalStore(onPhoneChange, isPhone, () => false);
}
