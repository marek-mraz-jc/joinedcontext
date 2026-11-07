/**
 * What changed for the people who use the Portal (T-3271): one entry per change a person can see,
 * newest first, written for them and not for developers. The words live in the locale files under
 * `whatsNew.entries.{key}`; `scripts/whats-new-candidates.py` lists the merged tasks an entry may
 * be written from, and a person writes and edits the entry here.
 */
export interface WhatsNewEntry {
  key: string;
  /** The day it reached the Portal, `YYYY-MM-DD`. */
  date: string;
  /** Where to see it, when it has a page of its own. */
  href?: string;
}

export const WHATS_NEW: WhatsNewEntry[] = [
  { key: "jobs", date: "2026-10-07" },
  { key: "history", date: "2026-10-07" },
  { key: "search", date: "2026-10-07" },
  { key: "shortcuts", date: "2026-10-07" },
  { key: "links", date: "2026-10-07" },
  { key: "home", date: "2026-10-07", href: "/" },
  { key: "glossary", date: "2026-10-07", href: "/glossary" },
  { key: "invite", date: "2026-10-07" },
  { key: "loadFile", date: "2026-10-07" },
  { key: "useData", date: "2026-10-07" },
  { key: "pipelineMapper", date: "2026-10-07" },
  { key: "knowledge", date: "2026-10-07" },
  { key: "mcp", date: "2026-10-07" },
  { key: "dataViews", date: "2026-10-06" },
];

const SEEN = "jc.whatsNewSeen";

/** The newest entry's day, or nothing when there are none. */
export const newest = (): string | undefined => WHATS_NEW[0]?.date;

/** The day this person last read the list; nothing when never or when the browser keeps nothing. */
export function lastSeen(): string | undefined {
  try {
    return localStorage.getItem(SEEN) ?? undefined;
  } catch {
    return undefined;
  }
}

export function markSeen(): void {
  const day = newest();
  if (!day) return;
  try {
    localStorage.setItem(SEEN, day);
  } catch {
    // A browser that keeps nothing shows the dot again next time, and nothing breaks.
  }
}

/** Whether an entry is newer than `seen`, the day last read; everything is, when nothing was read. */
export const isUnread = (entry: WhatsNewEntry, seen: string | undefined): boolean =>
  seen === undefined || entry.date > seen;
