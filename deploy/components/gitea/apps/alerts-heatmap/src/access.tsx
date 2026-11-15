import { createContext, useContext, useEffect, useState } from "react";
import type { ReactNode } from "react";
import type { Me } from "./server";

/** The App role that acknowledges alerts (app.yaml). */
export const STEWARD = "steward";

export interface Access {
  /** Who is signed in; `null` when signed out, `undefined` while it is asked. */
  me: Me | null | undefined;
  steward: boolean;
}

const AccessContext = createContext<Access>({ me: undefined, steward: false });

/** Who the reader is, asked of the App's server once. What it offers, the server decides again. */
export function AccessProvider({ api, children }: { api: { me: () => Promise<Me | null> }; children: ReactNode }) {
  const [me, setMe] = useState<Me | null | undefined>(undefined);
  useEffect(() => {
    let current = true;
    api.me().then(
      (who) => current && setMe(who),
      () => current && setMe(null),
    );
    return () => {
      current = false;
    };
  }, [api]);
  const steward = me?.roles.includes(STEWARD) ?? false;
  return <AccessContext.Provider value={{ me, steward }}>{children}</AccessContext.Provider>;
}

export function useAccess(): Access {
  return useContext(AccessContext);
}
