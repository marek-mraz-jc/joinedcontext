import { createContext, useCallback, useContext, useEffect, useState } from "react";
import type { ReactNode } from "react";
import type { Levels, Me, PlansApi } from "./plans";

/** The App role that changes every plan and the levels (app.yaml). */
export const STEWARD = "steward";

/** The planner's own levels (wasm/src/lib.rs `Settings::default`), until the server's arrive. */
export const DEFAULT_LEVELS: Levels = { low: 0.15, high: 0.85, target: 0.5 };

export interface Access {
  /** Who is signed in; `null` when signed out, `undefined` while it is asked. */
  me: Me | null | undefined;
  steward: boolean;
  levels: Levels;
  /** The server's levels arrived; until then `levels` are the planner's defaults. */
  levelsKept: boolean;
  /**
   * The levels saved: shown at once, put back as they were when the server refuses, which throws
   * its refusal on to the caller.
   */
  saveLevels: (next: Levels) => Promise<void>;
}

const AccessContext = createContext<Access>({ me: undefined, steward: false, levels: DEFAULT_LEVELS, levelsKept: false, saveLevels: async () => undefined });

/** Who the reader is and the levels every plan uses, asked of the App's server once. */
export function AccessProvider({ api, children }: { api: PlansApi; children: ReactNode }) {
  const [me, setMe] = useState<Me | null | undefined>(undefined);
  const [levels, setLevels] = useState<Levels>(DEFAULT_LEVELS);
  const [levelsKept, setLevelsKept] = useState(false);

  useEffect(() => {
    let current = true;
    api.me().then(
      (who) => current && setMe(who),
      () => current && setMe(null),
    );
    api.levels().then(
      (kept) => {
        if (!current) return;
        setLevels(kept);
        setLevelsKept(true);
      },
      () => undefined,
    );
    return () => {
      current = false;
    };
  }, [api]);

  const saveLevels = useCallback(
    async (next: Levels) => {
      const before = levels;
      setLevels(next);
      try {
        setLevels(await api.setLevels(next));
      } catch (error) {
        setLevels(before);
        throw error;
      }
    },
    [api, levels],
  );

  const steward = me?.roles.includes(STEWARD) ?? false;
  return <AccessContext.Provider value={{ me, steward, levels, levelsKept, saveLevels }}>{children}</AccessContext.Provider>;
}

export function useAccess(): Access {
  return useContext(AccessContext);
}
