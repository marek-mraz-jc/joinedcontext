import { useEffect, useMemo, useState } from "react";
import type { CellInspectorLabels } from "./CellInspector";
import type { AttributeAccess } from "./source";
import type { GridLabels } from "./useEntityGrid";

/**
 * The person's decisions per attribute: the host's, else the source's own (T-5528). Until they are
 * read, and when they cannot be, no cell opens an editor whose write could be refused. What the
 * person may not read (T-5530) is never asked for: it leaves the query's `attrs` and the columns,
 * and stands as a "hidden: no access" column instead, so nothing is silently missing.
 */
export function useGridAccess(hostAccess: AttributeAccess[] | null | undefined, askAccess: ((type: string) => Promise<AttributeAccess[]>) | undefined, accessType: string) {
  const [ownAccess, setOwnAccess] = useState<{ type: string; rows?: AttributeAccess[]; failed?: string } | null>(null);
  useEffect(() => {
    if (hostAccess !== undefined || !askAccess) return;
    let current = true;
    setOwnAccess(null);
    askAccess(accessType).then(
      (rows) => current && setOwnAccess({ type: accessType, rows }),
      (err: unknown) => current && setOwnAccess({ type: accessType, failed: err instanceof Error ? err.message : String(err) }),
    );
    return () => {
      current = false;
    };
  }, [hostAccess, askAccess, accessType]);
  const accessRows = hostAccess !== undefined ? hostAccess : ownAccess?.type === accessType ? ownAccess.rows : undefined;
  const unreadable = useMemo(() => (accessRows ?? []).filter((row) => !row.read).map((row) => row.name), [accessRows]);
  const ownFailed = ownAccess?.type === accessType ? ownAccess.failed : undefined;
  return { accessRows, unreadable, ownFailed };
}

/** Why the person cannot change an attribute, in the inspector's own words (T-5530). */
export function lockReason(row: AttributeAccess | undefined, labels: GridLabels, words: CellInspectorLabels): string {
  if (!row) return words.noGrant;
  if (row.write) return labels.lockNotOffered;
  switch (row.reason) {
    case "prohibited":
      return words.prohibited;
    case "formula":
      return words.formula;
    case "not_in_grant":
      return words.notInGrant;
    case "no_grant":
      return words.noGrant;
    default:
      return labels.lockReadOnly;
  }
}


export function noRow(attr: string, rows: AttributeAccess[] | null | undefined): AttributeAccess | undefined {
  return rows ? { name: attr, read: false, write: false, delete: false, reason: "no_grant" } : undefined;
}
