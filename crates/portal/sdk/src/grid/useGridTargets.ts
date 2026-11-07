import React, { useCallback, useEffect, useMemo, useState } from "react";
import { targetName } from "./cells";
import type { ResolvedTargets } from "./cells";
import type { RichRow } from "./model";
import { NGSI_LD_NULL } from "./RelationPicker";
import { objectsOf, pointingAt, resolveTargets, searchTargets } from "../relations";
import type { TargetOption } from "../relations";
import type { GridLabels, UseEntityGridOptions } from "./useEntityGrid";

const NO_TARGETS: ResolvedTargets = new Map();

/** What the page's relationships point at: both ends' names, and the way to open or search them. */
export function useGridTargets(
  options: UseEntityGridOptions,
  rows: RichRow[],
  labels: GridLabels,
  onOpenRelationship: ((urn: string) => void) | undefined,
  onDrill: ((urn: string, type: string) => void) | undefined,
) {
  // The computed ends of the page: one read per end for every row on it, through the grid's own
  // source, so only what the person may read is listed (DM-67, UI-84).
  const relations = options.relations;
  const computed = useMemo(
    () => Object.entries(relations ?? {}).filter(([, end]) => end.inverseOf !== undefined),
    [relations],
  );
  const pageIds = rows.map((row) => row.id).join("\n");
  const [inverse, setInverse] = useState<Record<string, { byId: Record<string, TargetOption[]>; full: boolean } | null>>({});
  React.useEffect(() => {
    if (computed.length === 0 || pageIds === "") {
      setInverse({});
      return;
    }
    let live = true;
    const ids = pageIds.split("\n");
    Promise.all(
      computed.map(([attr, end]) =>
        pointingAt(options.source, end.target, end.inverseOf!, ids).then(
          (found) => [attr, found] as const,
          () => [attr, null] as const,
        ),
      ),
    ).then((all) => {
      if (live) setInverse(Object.fromEntries(all));
    });
    return () => {
      live = false;
    };
  }, [computed, pageIds, options.source]);
  // The targets of the page's relationships, read once per page as one query per type and never
  // per cell (G-R2GRID2-017). Only a target that comes back is a link, named by its name; the rest
  // were deleted or are not readable, and read as their id.
  const wantedTargets = useMemo(() => {
    const wanted = new Map<string, string>();
    for (const row of rows) {
      for (const [attr, cell] of Object.entries(row.cells)) {
        for (const urn of objectsOf(cell)) {
          const type = targetName(urn).type ?? relations?.[attr]?.target;
          if (type && urn !== NGSI_LD_NULL) wanted.set(urn, type);
        }
      }
    }
    return wanted;
  }, [rows, relations]);
  const wantedKey = [...wantedTargets.keys()].join("\n");
  const [resolved, setResolved] = useState<{ key: string; found: ResolvedTargets }>({ key: "", found: NO_TARGETS });
  useEffect(() => {
    if (wantedTargets.size === 0) return;
    let live = true;
    void resolveTargets(options.source, wantedTargets).then((found) => {
      if (live) setResolved({ key: [...wantedTargets.keys()].join("\n"), found });
    });
    return () => {
      live = false;
    };
  }, [wantedTargets, options.source]);
  const targets = resolved.key === wantedKey ? resolved.found : NO_TARGETS;
  const canOpen = Boolean(onOpenRelationship || onDrill);
  const openTarget = useCallback(
    (urn: string, type?: string) => {
      if (onOpenRelationship) {
        onOpenRelationship(urn);
        return;
      }
      const kind = type ?? targets.get(urn)?.type;
      if (onDrill && kind) onDrill(urn, kind);
    },
    [onOpenRelationship, onDrill, targets],
  );
  // One search per target class for as long as the source stays: a new function each render
  // would ask the endpoint again every time the grid draws.
  const searchOf = useMemo(() => {
    const made = new Map<string, (text: string) => Promise<TargetOption[]>>();
    return (target: string) => {
      let search = made.get(target);
      if (!search) {
        search = (text: string) => searchTargets(options.source, target, text);
        made.set(target, search);
      }
      return search;
    };
  }, [options.source]);
  const pickerLabels = useMemo(
    () => ({
      search: labels.relationSearch,
      none: labels.relationNone,
      remove: labels.relationRemove,
      loading: labels.loading,
      failed: labels.relationFailed,
    }),
    [labels.relationSearch, labels.relationNone, labels.relationRemove, labels.loading, labels.relationFailed],
  );
  return { relations, inverse, targets, canOpen, openTarget, searchOf, pickerLabels };
}
