import { useCallback, useEffect, useRef, useState } from "react";
import type { FeaturedEvent, ShareApi } from "./share";

export interface Featured {
  /** The featured events, `null` while they are read. */
  list: FeaturedEvent[] | null;
  /** Why they could not be read, if they could not. */
  failed: unknown;
  /** An event featured: shown at once, taken back when the server refuses (which throws on). */
  add: (eventId: string) => Promise<void>;
  /** A note changed, the same way. */
  setNote: (id: number, note: string) => Promise<void>;
  /** An event no longer featured, the same way. */
  remove: (id: number) => Promise<void>;
}

/** The featured events every visitor sees; a steward's changes show before the server answers. */
export function useFeatured(api: ShareApi): Featured {
  const [list, setShown] = useState<FeaturedEvent[] | null>(null);
  const [failed, setFailed] = useState<unknown>(null);
  // What is on screen now, read by a change that has to put it back.
  const shown = useRef<FeaturedEvent[]>([]);
  const show = useCallback((next: FeaturedEvent[]) => {
    shown.current = next;
    setShown(next);
  }, []);

  useEffect(() => {
    let current = true;
    api.featured().then(
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
  }, [api, show]);

  /** `change` shown at once, then `work`; what was shown before comes back when it throws. */
  const optimistic = async (change: FeaturedEvent[], work: () => Promise<void>) => {
    const before = shown.current;
    show(change);
    try {
      await work();
    } catch (error) {
      show(before);
      throw error;
    }
  };

  const add = (eventId: string) =>
    optimistic([...shown.current, { id: -1, eventId, note: "" }], async () => {
      const made = await api.feature(eventId, "");
      show(shown.current.map((one) => (one.id === -1 && one.eventId === eventId ? made : one)));
    });
  const setNote = (id: number, note: string) =>
    optimistic(
      shown.current.map((one) => (one.id === id ? { ...one, note: note.trim() } : one)),
      () => api.changeNote(id, note.trim()),
    );
  const remove = (id: number) =>
    optimistic(
      shown.current.filter((one) => one.id !== id),
      () => api.unfeature(id),
    );

  return { list, failed, add, setNote, remove };
}
