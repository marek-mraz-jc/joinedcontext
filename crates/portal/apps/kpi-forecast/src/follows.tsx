import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { useAccess } from "./access";
import type { Follow, KpiApi } from "./server";

export interface Follows {
  /** The reader's follows; `null` while read or signed out. */
  list: Follow[] | null;
  failed: unknown;
  /** Kept on the server first, as it gets its id there. */
  follow: (kpi: string) => Promise<void>;
  /** Gone at once, back when the server refuses (which throws on). */
  unfollow: (id: number) => Promise<void>;
}

const FollowsContext = createContext<Follows | null>(null);

/** The indicators the signed-in reader follows (T-3827), read once they are known. */
export function FollowsProvider({ api, children }: { api: KpiApi; children: ReactNode }) {
  const { me } = useAccess();
  const [list, setShown] = useState<Follow[] | null>(null);
  const [failed, setFailed] = useState<unknown>(null);
  const shown = useRef<Follow[]>([]);
  const show = useCallback((next: Follow[]) => {
    shown.current = next;
    setShown(next);
  }, []);

  useEffect(() => {
    if (!me) return;
    let current = true;
    api.follows().then(
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
  }, [api, me, show]);

  const follow = useCallback(
    async (kpi: string) => {
      const made = await api.follow(kpi);
      show([made, ...shown.current]);
    },
    [api, show],
  );
  const unfollow = useCallback(
    async (id: number) => {
      const before = shown.current;
      show(before.filter((one) => one.id !== id));
      try {
        await api.unfollow(id);
      } catch (error) {
        show(before);
        throw error;
      }
    },
    [api, show],
  );
  return <FollowsContext.Provider value={{ list, failed, follow, unfollow }}>{children}</FollowsContext.Provider>;
}

export function useFollows(): Follows {
  const follows = useContext(FollowsContext);
  if (!follows) throw new Error("useFollows needs a FollowsProvider above it");
  return follows;
}
