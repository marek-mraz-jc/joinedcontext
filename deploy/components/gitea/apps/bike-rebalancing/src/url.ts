import { useCallback, useState } from "react";

/** The page's filters, kept in the address so a reload or a shared link shows the same view. */
export function readParams(search: string): URLSearchParams {
  return new URLSearchParams(search);
}

/** `params` written back into the address, keeping its path and its `#page`; a sandbox that refuses is ignored. */
export function writeParams(params: URLSearchParams): void {
  try {
    const query = params.toString();
    window.history.replaceState(null, "", `${window.location.pathname}${query ? `?${query}` : ""}${window.location.hash}`);
  } catch {
    // A sandboxed preview may refuse to touch the address; the view still changes.
  }
}

/** One parameter of the address as state: read once, written on every change; `""` removes it. */
export function useParam(name: string, fallback = ""): [string, (value: string) => void] {
  const [value, setValue] = useState(() => readParams(window.location.search).get(name) ?? fallback);
  const set = useCallback(
    (next: string) => {
      setValue(next);
      const params = readParams(window.location.search);
      if (next === "" || next === fallback) params.delete(name);
      else params.set(name, next);
      writeParams(params);
    },
    [name, fallback],
  );
  return [value, set];
}

/** A comma-separated list in the address, as a set of ids. */
export function listOf(value: string): string[] {
  return value.split(",").map((part) => part.trim()).filter(Boolean);
}
