import { useCallback, useEffect, useState } from "react";
import { ProblemError, useClient } from "@joinedcontext/sdk";
import type { TemporalRow } from "@joinedcontext/sdk";
import { ATTRS, LAST_N, VEHICLE } from "./vehicles";

const HOUR = 3_600_000;

/**
 * Every vehicle's position, speed and route over the last `hours` hours, from the temporal read of
 * the App's endpoint. A refused or failed read is an error beside an empty history: the page then
 * answers with walking alone.
 */
export function useHistory(hours: number, enabled: boolean): { history: TemporalRow[]; loading: boolean; error: ProblemError | null; reload: () => void } {
  const client = useClient();
  const [state, setState] = useState<{ history: TemporalRow[]; loading: boolean; error: ProblemError | null }>({
    // Loading until the first read answers, even before it may start: a page that analysed the
    // empty history meanwhile flashed a walking-only answer (as kpi-forecast, T-3400).
    history: [],
    loading: true,
    error: null,
  });
  const [nonce, setNonce] = useState(0);
  const reload = useCallback(() => setNonce((n) => n + 1), []);
  useEffect(() => {
    if (!enabled) return;
    let current = true;
    setState((previous) => ({ ...previous, loading: true, error: null }));
    client.temporal
      .list(VEHICLE, { attrs: ATTRS, timerel: "after", timeAt: new Date(Date.now() - hours * HOUR).toISOString(), lastN: LAST_N })
      .then(
        (history) => current && setState({ history, loading: false, error: null }),
        (error: unknown) =>
          current &&
          setState({
            history: [],
            loading: false,
            error: error instanceof ProblemError ? error : new ProblemError(0, { title: error instanceof Error ? error.message : String(error) }),
          }),
      );
    return () => {
      current = false;
    };
  }, [client, hours, enabled, nonce]);
  return { ...state, reload };
}
