import { useEffect, useState } from "react";

/**
 * A dialog's module, imported the first time the dialog opens (T-3316) and kept in `cache` for
 * every later one; true once `cache.current` holds it. Not `React.lazy`: under its Suspense
 * boundary the form's first change rendered and never committed, so Check ran on the form as it
 * was before (the form_validation suites caught it). A module that fails to load is thrown to the
 * nearest error boundary rather than leaving the dialog silently shut.
 */
export function useLoadedWhenOpen<C>(open: boolean, cache: { current?: C }, load: () => Promise<C>): boolean {
  const [ready, setReady] = useState(() => cache.current !== undefined);
  const [failed, setFailed] = useState<Error | null>(null);
  useEffect(() => {
    if (!open || ready) return undefined;
    let live = true;
    load().then(
      (loaded) => {
        cache.current = loaded;
        if (live) setReady(true);
      },
      (error: unknown) => {
        if (live) setFailed(error instanceof Error ? error : new Error(String(error)));
      },
    );
    return () => {
      live = false;
    };
  }, [open, ready, cache, load]);
  if (failed) throw failed;
  return ready;
}
