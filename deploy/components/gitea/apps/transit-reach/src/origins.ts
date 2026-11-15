import { useCallback, useEffect, useRef, useState } from "react";
import { t } from "./i18n";
import type { Lang, Word } from "./i18n";
import { ServerProblem } from "./server";
import type { Origin, ReachApi } from "./server";

/** What the server's refusal of a change means, in the reader's language; `failed` heads a refusal it explains. */
export function changeProblem(lang: Lang, error: unknown, failed: Word): string {
  if (!(error instanceof ServerProblem)) return t(lang, "serverDown");
  const words: Partial<Record<number, Word>> = { 0: "serverOffline", 401: "signInAgain", 403: "noRight", 404: "gone", 507: "storageFull" };
  const word = words[error.status];
  if (word) return t(lang, word);
  if ([400, 409, 422].includes(error.status)) return `${t(lang, failed)}: ${error.message}`;
  return t(lang, "serverDown");
}

export interface Origins {
  /** The origins the reader may see, `null` while they are read. */
  list: Origin[] | null;
  /** Why they could not be read, if they could not. */
  failed: unknown;
  reload: () => void;
  /** Renamed: shown at once, taken back when the server refuses (which throws on). */
  rename: (id: number, name: string) => Promise<void>;
  /** Deleted, the same way. */
  remove: (id: number) => Promise<void>;
  /** In or out of the shared set, the same way. */
  publish: (id: number, published: boolean) => Promise<void>;
}

/** The origins of the reader, the published ones and (for a steward) everyone's; a change shows before the server answers. */
export function useOrigins(api: ReachApi, enabled = true): Origins {
  const [list, setShown] = useState<Origin[] | null>(null);
  const [failed, setFailed] = useState<unknown>(null);
  const [nonce, setNonce] = useState(0);
  // What is on screen now, read by a change that has to put it back.
  const shown = useRef<Origin[]>([]);
  const show = useCallback((next: Origin[]) => {
    shown.current = next;
    setShown(next);
  }, []);

  useEffect(() => {
    if (!enabled) return;
    let current = true;
    setFailed(null);
    api.origins().then(
      (found) => current && show(found),
      (error: unknown) => {
        if (!current) return;
        show([]);
        setFailed(error);
      },
    );
    return () => {
      current = false;
    };
  }, [api, show, enabled, nonce]);

  const optimistic = async (change: Origin[], work: () => Promise<void>) => {
    const before = shown.current;
    show(change);
    try {
      await work();
    } catch (error) {
      show(before);
      throw error;
    }
  };
  const changed = (id: number, patch: Partial<Origin>) => shown.current.map((one) => (one.id === id ? { ...one, ...patch } : one));

  return {
    list,
    failed,
    reload: () => setNonce((n) => n + 1),
    rename: (id, name) => optimistic(changed(id, { name: name.trim() }), () => api.rename(id, name.trim())),
    remove: (id) =>
      optimistic(
        shown.current.filter((one) => one.id !== id),
        () => api.forget(id),
      ),
    publish: (id, published) => optimistic(changed(id, { published }), () => api.publish(id, published)),
  };
}
