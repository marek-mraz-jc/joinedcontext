import type { FnHandler, Row } from "@joinedcontext/sdk/server";

export interface Summary {
  total: number;
  byCategory: Record<string, number>;
  bySubCategory: Record<string, number>;
  /** The open alert issued first: no end, or an end still ahead. */
  oldestOpen: { id: string; name: string | null; dateIssued: string } | null;
  /** For a steward only: the alerts a steward added, the ones they may remove. */
  ownRecords?: number;
}

const MAX_ROWS = 5000;

function countBy(rows: Row[], attr: string): Record<string, number> {
  const counts: Record<string, number> = {};
  for (const row of rows) {
    const key = typeof row[attr] === "string" && row[attr] !== "" ? (row[attr] as string) : "unknown";
    counts[key] = (counts[key] ?? 0) + 1;
  }
  return counts;
}

function isOpen(row: Row, now: number): boolean {
  if (row.validTo === null || row.validTo === undefined || row.validTo === "") return true;
  const end = Date.parse(String(row.validTo));
  return Number.isNaN(end) || end > now;
}

/** Counts per category and subCategory and the oldest open alert, read with the caller's own grants. */
const summary: FnHandler = async (request, ctx) => {
  const rows = await ctx.jc.entities.all("Alert", { limit: MAX_ROWS });
  const now = Date.now();
  let oldest: Row | null = null;
  for (const row of rows) {
    if (!isOpen(row, now) || typeof row.dateIssued !== "string") continue;
    if (!oldest || Date.parse(row.dateIssued) < Date.parse(String(oldest.dateIssued))) oldest = row;
  }
  const body: Summary = {
    total: rows.length,
    byCategory: countBy(rows, "category"),
    bySubCategory: countBy(rows, "subCategory"),
    oldestOpen: oldest
      ? { id: oldest.id, name: typeof oldest.name === "string" ? oldest.name : null, dateIssued: String(oldest.dateIssued) }
      : null,
  };
  // AP-93, SDK-21: the host sets request.user from the person's app roles; a viewer never sees
  // the steward's number, not even as zero.
  if (request.user?.roles?.includes("steward")) {
    body.ownRecords = rows.filter((row) => row.source === null || row.source === undefined || row.source === "").length;
  }
  ctx.log("summary", rows.length, "alerts");
  return { status: 200, body };
};

export default summary;
