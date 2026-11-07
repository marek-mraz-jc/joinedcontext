import { useCallback, useEffect, useRef, useState } from "react";
import { ZONE } from "./i18n";
import type { AirApi, Fault, Hourly } from "./server";

/** Helsinki's offset from UTC at the instant `ms`, in milliseconds. */
export function offsetAt(ms: number): number {
  const parts = Object.fromEntries(
    new Intl.DateTimeFormat("en-GB", { timeZone: ZONE, hourCycle: "h23", year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" })
      .formatToParts(ms)
      .map((part) => [part.type, Number(part.value)]),
  );
  const wall = Date.UTC(parts.year, parts.month - 1, parts.day, parts.hour, parts.minute);
  return wall - Math.floor(ms / 60_000) * 60_000;
}

/** A `datetime-local` value read as Helsinki's wall clock; `null` when it names no time. */
export function helsinkiMs(text: string): number | null {
  const wall = Date.parse(`${text}:00Z`);
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(text) || Number.isNaN(wall)) return null;
  // The offset at the wall time's own instant: twice, so a change of season between is taken.
  const first = wall - offsetAt(wall);
  return wall - offsetAt(first);
}

/** The instant `ms` as a `datetime-local` value on Helsinki's wall clock. */
export function helsinkiWall(ms: number): string {
  return new Date(ms + offsetAt(ms)).toISOString().slice(0, 16);
}

/** The station's hours with every hour a mark covers left out, and how many hours went. */
export function withoutFaults(hourly: Hourly, faults: Fault[]): { hourly: Hourly; leftOut: number } {
  if (faults.length === 0) return { hourly, leftOut: 0 };
  const faulty = (ms: number) => faults.some((fault) => ms >= fault.from && ms < fault.to);
  const gone = new Set<number>();
  const kept: Hourly = {};
  for (const [attr, points] of Object.entries(hourly)) {
    kept[attr] = points.filter(([ms]) => {
      if (!faulty(ms)) return true;
      gone.add(ms);
      return false;
    });
  }
  return { hourly: kept, leftOut: gone.size };
}

export interface Faults {
  /** The station's marks, `null` while they are read. */
  list: Fault[] | null;
  failed: unknown;
  /** A mark added: kept on the server first, as it gets its id there. */
  mark: (fault: Omit<Fault, "id">) => Promise<void>;
  /** A mark taken back: gone at once, back when the server refuses (which throws on). */
  unmark: (id: number) => Promise<void>;
}

/** The marks of the station `station`, read again when it changes. */
export function useFaults(api: AirApi, station: string | undefined): Faults {
  const [list, setShown] = useState<Fault[] | null>(null);
  const [failed, setFailed] = useState<unknown>(null);
  const shown = useRef<Fault[]>([]);
  const show = useCallback((next: Fault[]) => {
    shown.current = next;
    setShown(next);
  }, []);

  useEffect(() => {
    if (!station) return;
    let current = true;
    setShown(null);
    setFailed(null);
    api.faults(station).then(
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
  }, [api, station, show]);

  return {
    list,
    failed,
    mark: async (fault) => {
      const made = await api.markFault(fault);
      show([made, ...shown.current]);
    },
    unmark: async (id) => {
      const before = shown.current;
      show(before.filter((one) => one.id !== id));
      try {
        await api.unmarkFault(id);
      } catch (error) {
        show(before);
        throw error;
      }
    },
  };
}
