import { requireRole } from "@joinedcontext/sdk/server";
import type { FnHandler } from "@joinedcontext/sdk/server";

/** A report's id as the endpoint mints it; anything else is not looked up. */
const REPORT = /^urn:ngsi-ld:DefectReport:[A-Za-z0-9._:~-]{1,200}$/;

/**
 * Closes one report. The role is checked here, on the person the Portal verified for this call:
 * a page that shows the button to everyone changes nothing.
 */
const closeReport: FnHandler = async (request, ctx) => {
  const refused = requireRole(request, "steward");
  if (refused) return refused;
  const id: unknown = request.body?.id;
  if (typeof id !== "string" || !REPORT.test(id)) {
    return { status: 400, body: { title: "id must be the id of one DefectReport" } };
  }
  await ctx.jc.entities.update(id, { status: "closed" });
  ctx.log("closed", id);
  return { status: 200, body: { id, status: "closed" } };
};

export default closeReport;
