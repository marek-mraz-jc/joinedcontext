import { useCallback, useEffect, useRef, useState } from "react";
import type { Resolution, Server } from "./server";

/** The key of a finding a resolution answers: its entity and attribute. */
export const findingKey = (entity: string, attribute: string) => `${entity}\n${attribute}`;

export interface Resolutions {
  /** By finding; empty while read. */
  byFinding: Map<string, Resolution>;
  failed: unknown;
  resolve: (entity: string, attribute: string, note: string) => Promise<void>;
  correct: (entity: string, attribute: string, value: string | number | boolean, note: string) => Promise<void>;
  /** Open at once, resolved again when the server refuses (which throws on). */
  reopen: (id: number) => Promise<void>;
}

/** What stewards resolved (T-3827), read once, and their changes to it. */
export function useResolutions(server: Server): Resolutions {
  const [list, setShown] = useState<Resolution[]>([]);
  const [failed, setFailed] = useState<unknown>(null);
  const shown = useRef<Resolution[]>([]);
  const show = useCallback((next: Resolution[]) => {
    shown.current = next;
    setShown(next);
  }, []);

  useEffect(() => {
    let current = true;
    server.resolutions().then(
      (found) => current && show(found),
      (error: unknown) => current && setFailed(error),
    );
    return () => {
      current = false;
    };
  }, [server, show]);

  const byFinding = new Map<string, Resolution>();
  // The newest first, so an older resolution of the same finding never hides it.
  for (const r of list) if (!byFinding.has(findingKey(r.entity, r.attribute))) byFinding.set(findingKey(r.entity, r.attribute), r);

  return {
    byFinding,
    failed,
    resolve: async (entity, attribute, note) => {
      show([await server.resolve(entity, attribute, note), ...shown.current]);
    },
    correct: async (entity, attribute, value, note) => {
      const made = await server.correct(entity, attribute, value, note);
      if (made) show([made, ...shown.current]);
    },
    reopen: async (id) => {
      const before = shown.current;
      show(before.filter((r) => r.id !== id));
      try {
        await server.reopen(id);
      } catch (error) {
        show(before);
        throw error;
      }
    },
  };
}
