import { useCallback, useMemo, useState } from "react";
import { applyChanges, MAX_REASON } from "./apply";
import type { EntityChange, Observed, Refusal } from "./apply";
import { problemOf } from "./rules";
import type { ValueRule } from "./rules";
import { NGSI_LD_NULL } from "./RelationPicker";
import type { EntitySource } from "./source";
import type { EntityGrid as EntityGridApi } from "./useEntityGrid";

/** The pending values on their way to the source: Review, Apply, Undo and what each cell was refused. */
export function useGridEdits(options: { source: EntitySource }, grid: EntityGridApi, rules: Record<string, ValueRule> | undefined) {
  const { labels, state, pendingChanges, setEdits, reload } = grid;
  const [open, setOpen] = useState(false);
  const [observed, setObserved] = useState<Observed>("keep");
  const [applying, setApplying] = useState(false);
  // Why the person writes, sent with each update (DM-113): a default they can change, never empty.
  const [reason, setReason] = useState<string | null>(null);
  const reasonText = reason ?? labels.reasonDefault;
  // What the last apply wrote, with the values it replaced, so Undo can write those back.
  const [lastApplied, setLastApplied] = useState<{ entities: EntityChange[]; reason: string } | null>(null);
  const [undoNote, setUndoNote] = useState<string | null>(null);
  // What the last paste did, said once in a status line (T-3097).
  const [pasteNote, setPasteNote] = useState<string | null>(null);
  const [refused, setRefused] = useState<Refusal[]>([]);
  // What the pending values break of their model's rules, by `id\u0000attr` (T-3097).
  const ruleProblems = useMemo(() => {
    const found = new Map<string, string>();
    for (const [id, attrs] of Object.entries(state.edits)) {
      for (const [attr, value] of Object.entries(attrs)) {
        const problem = problemOf(rules?.[attr], value, labels);
        if (problem) found.set(`${id}\u0000${attr}`, problem);
      }
    }
    return found;
  }, [state.edits, rules, labels]);
  const refusedOf = useMemo(() => new Map(refused.map((one) => [one.id, one.detail])), [refused]);
  // A refusal that names its attribute belongs on that cell, beside the value (DM-70); one that
  // names none, a 403 of the Policy, on every cell of the entity still pending, since each of
  // them is what did not land.
  const refusedCell = useMemo(
    () =>
      new Map(
        refused.flatMap((one) =>
          (one.slot ? [one.slot] : Object.keys(state.edits[one.id] ?? {})).map(
            (attr) => [`${one.id}\u0000${attr}`, one.detail] as const,
          ),
        ),
      ),
    [refused, state.edits],
  );

  const rowTitle = useCallback(
    (id: string): string | undefined => {
      const detail = refusedOf.get(id);
      return detail ? `${labels.refusedHere}: ${detail}` : undefined;
    },
    [refusedOf, labels.refusedHere],
  );

  /**
   * Applies the pending cells through the endpoint. What landed is forgotten and re-read, so the
   * grid shows the endpoint's own answer; what was refused stays pending with its reason, because
   * the person's value is the only copy of it (UI-67).
   */
  const apply = useCallback(async () => {
    setApplying(true);
    try {
      const result = await applyChanges({
        source: options.source,
        entities: pendingChanges,
        reason: reasonText,
        observed,
        fallback: labels.error,
      });
      const landed = pendingChanges.filter((entity) => result.applied.includes(entity.id));
      setLastApplied(landed.length > 0 ? { entities: landed, reason: reasonText.trim() } : null);
      setUndoNote(null);
      // Every applied value leaves the pending list in one step: one call per value would keep
      // all but the last pending.
      setEdits(
        result.applied.flatMap((id) =>
          Object.keys(state.edits[id] ?? {}).map((attr) => ({ id, attr, value: undefined })),
        ),
      );
      setRefused(result.refused);
      if (result.refused.length === 0) {
        setOpen(false);
      }
      reload();
    } finally {
      setApplying(false);
    }
  }, [options.source, pendingChanges, reasonText, observed, labels.error, state.edits, setEdits, reload]);

  /**
   * Writes back what the last apply replaced, as a correction with its own reason (DM-113): an
   * attribute the apply added is written as NGSI-LD's null, so it goes again. What the endpoint
   * refuses is said at its row, as any refusal.
   */
  const undo = useCallback(async () => {
    if (!lastApplied) return;
    setApplying(true);
    try {
      const back: EntityChange[] = lastApplied.entities.map((entity) => ({
        id: entity.id,
        changes: entity.changes.map((change) => ({ ...change, before: change.after, after: change.before ?? NGSI_LD_NULL })),
      }));
      const result = await applyChanges({
        source: options.source,
        entities: back,
        reason: `${labels.undoReason}: ${lastApplied.reason}`.slice(0, MAX_REASON),
        fallback: labels.error,
      });
      setRefused(result.refused);
      setLastApplied(null);
      if (result.refused.length === 0) setUndoNote(labels.undone);
      reload();
    } finally {
      setApplying(false);
    }
  }, [lastApplied, options.source, labels.undoReason, labels.error, labels.undone, reload]);
  return { open, setOpen, observed, setObserved, applying, reasonText, setReason, lastApplied, undoNote, pasteNote, setPasteNote, refused, setRefused, ruleProblems, refusedOf, refusedCell, rowTitle, apply, undo };
}
