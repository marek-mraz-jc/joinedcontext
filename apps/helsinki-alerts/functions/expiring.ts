import type { FnHandler } from "@joinedcontext/sdk/server";

export interface Expiring {
  hours: number;
  alerts: Array<{ id: string; name: string | null; validTo: string }>;
}

const MAX_HOURS = 168;
const HOUR = 3600 * 1000;

/** The alerts whose `validTo` falls in the next N hours, soonest first; N is 1 to 168. */
const expiring: FnHandler = async (request, ctx) => {
  const given: unknown = request.method === "POST" ? request.body?.hours : request.query.hours;
  // A query string is text; a JSON body carries the number itself.
  const hours = request.method === "GET" && typeof given === "string" && /^\d+$/.test(given) ? Number(given) : given;
  if (typeof hours !== "number" || !Number.isInteger(hours) || hours < 1 || hours > MAX_HOURS) {
    return { status: 400, body: { title: `hours must be a whole number from 1 to ${MAX_HOURS}` } };
  }
  const now = Date.now();
  const until = now + hours * HOUR;
  const rows = await ctx.jc.entities.all("Alert", { limit: 5000 });
  const alerts = rows
    .flatMap((row) => {
      const end = typeof row.validTo === "string" ? Date.parse(row.validTo) : Number.NaN;
      return end > now && end <= until ? [{ id: row.id, name: typeof row.name === "string" ? row.name : null, validTo: String(row.validTo), end }] : [];
    })
    .sort((a, b) => a.end - b.end)
    .map(({ id, name, validTo }) => ({ id, name, validTo }));
  ctx.log("expiring", hours, alerts.length);
  return { status: 200, body: { hours, alerts } satisfies Expiring };
};

export default expiring;
