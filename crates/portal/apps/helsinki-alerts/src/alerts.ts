import type { Row } from "@joinedcontext/sdk";

export const ALERT = "Alert";

/** The columns of the table and the detail, in the order a reader scans them. */
export const COLUMNS = ["name", "category", "subCategory", "address", "validFrom", "validTo", "dateIssued"];

/** What the steward's form writes: the `dataNeeds` item of `app.yaml` granted to `steward`. */
export const WRITABLE = ["name", "description", "category", "subCategory", "address", "dateIssued", "validFrom", "validTo", "location"];

/** The attributes the form shows but never writes, and why. */
export const READ_ONLY: Array<[string, string]> = [
  ["id", "the platform names a record once, when it is created"],
  ["type", "every record here is an Alert"],
  ["observedAt", "set by the feed that observed it"],
  ["source", "where Fintraffic published it; a steward's own record has none"],
];

/** A record a steward added: Fintraffic's always name their source, and only these may be removed. */
export function isOwn(row: Row): boolean {
  return row.source === null || row.source === undefined || row.source === "";
}
