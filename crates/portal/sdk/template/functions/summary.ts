import type { FnHandler, Row } from "@joinedcontext/sdk/server";

export interface TypeSummary {
  type: string;
  count: number;
  /** The mean of every attribute whose values are all numbers, rounded to two decimals. */
  averages: Record<string, number>;
}

export interface Summary {
  types: TypeSummary[];
}

const TYPE = /^[A-Za-z][A-Za-z0-9_]*$/;
const MAX_TYPES = 20;

function averagesOf(rows: Row[]): Record<string, number> {
  const sums = new Map<string, { total: number; count: number; numeric: boolean }>();
  for (const row of rows) {
    for (const [attr, value] of Object.entries(row)) {
      if (attr === "id" || attr === "type" || value === null) continue;
      const entry = sums.get(attr) ?? { total: 0, count: 0, numeric: true };
      if (typeof value === "number" && Number.isFinite(value)) {
        entry.total += value;
        entry.count += 1;
      } else {
        entry.numeric = false;
      }
      sums.set(attr, entry);
    }
  }
  const averages: Record<string, number> = {};
  for (const [attr, { total, count, numeric }] of sums) {
    if (numeric && count > 0) averages[attr] = Math.round((total / count) * 100) / 100;
  }
  return averages;
}

/** Counts and averages per entity type, read through the endpoint with the caller's grants. */
const summary: FnHandler = async (request, ctx) => {
  const given: unknown = request.method === "POST" ? request.body?.types : request.query.types?.split(",");
  if (!Array.isArray(given) || given.length === 0 || given.length > MAX_TYPES || !given.every((t) => typeof t === "string" && TYPE.test(t))) {
    return { status: 400, body: { title: `types must be 1 to ${MAX_TYPES} entity type names` } };
  }
  // A type two endpoints serve is read from the one the page names (SDK-02).
  const endpoint: unknown = request.method === "POST" ? request.body?.endpoint : request.query.endpoint;
  if (endpoint !== undefined && (typeof endpoint !== "string" || endpoint === "")) {
    return { status: 400, body: { title: "endpoint must be the name of one endpoint" } };
  }
  const types: TypeSummary[] = [];
  for (const type of given as string[]) {
    const rows = await ctx.jc.entities.all(type, { limit: 5000, ...(endpoint ? { endpoint } : {}) });
    types.push({ type, count: rows.length, averages: averagesOf(rows) });
  }
  ctx.log("summary", types.length, "types");
  return { status: 200, body: { types } satisfies Summary };
};

export default summary;
