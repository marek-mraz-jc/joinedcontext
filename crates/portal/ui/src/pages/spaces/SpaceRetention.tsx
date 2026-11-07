import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import type { Manifest, ResourceProposal } from "../../api/manifest";
import { proposeChecked } from "../../api/proposal";
import {
  Alert,
  Button,
  ConfirmDialog,
  Input,
  PermissionGuard,
  Select,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";
import { RetentionGuide } from "../../components/RetentionGuide";
import {
  ANY_TYPE,
  effectiveCurrent,
  effectiveHistory,
  fromIso,
  HISTORY_FOREVER,
  HISTORY_OFF,
  RECORD_DEFAULT,
  RECORD_ON_CHANGE,
  RECORDS,
  rulesOf,
  shortens,
  toIso,
  UNITS,
  withRecord,
  written,
} from "./retention";
import { HEARTBEATS } from "../pipelines/history";
import type { Effective, RetentionRule, Unit } from "./retention";

type Count = components["schemas"]["RetentionCount"];

/** One duration as the editor holds it: what the person typed, forever, or history off. */
interface Cell {
  amount: string;
  unit: Unit | "forever" | "off";
  /** The stored duration, written back as it was while the cell shows it unchanged. */
  stored?: string;
}

function cellOf(iso?: string): Cell {
  if (iso === HISTORY_OFF) return { amount: "", unit: "off", stored: iso };
  const keep = iso === undefined ? undefined : fromIso(iso);
  return keep === undefined
    ? { amount: "", unit: "forever", stored: iso }
    : { amount: String(keep.amount), unit: keep.unit, stored: iso };
}

/** The manifest's duration of a cell; `null` when the person typed something the broker refuses. */
function isoOf(cell: Cell): string | undefined | null {
  if (cell.unit === "off") return HISTORY_OFF;
  // A history the manifest keeps forever with its reason stays as written (T-4275).
  if (cell.unit === "forever") return cell.stored === HISTORY_FOREVER ? HISTORY_FOREVER : undefined;
  const iso = toIso({ amount: Number(cell.amount), unit: cell.unit });
  if (iso === undefined) return null;
  const stored = cell.stored === undefined ? undefined : fromIso(cell.stored);
  return stored !== undefined && toIso(stored) === iso ? cell.stored : iso;
}

/** A type's row as the editor holds it: both durations, the record mode and its heartbeat. */
interface Draft {
  current: Cell;
  history: Cell;
  record: string;
  heartbeat: string;
}

/**
 * How long a space keeps each type's data (DM-82, DM-85, ADR-N-046): the rules in plain units,
 * what the broker's sweeps deleted, and for the space's owners an editor that names what a shorter
 * rule deletes before it is proposed. A type whose rule says nothing of history takes the space's
 * default, the row "*", and without either keeps no history: the platform runs the broker with
 * history opt-in (DM-132). Each type also says which rows of its history the broker writes.
 */
export function SpaceRetention({
  project,
  space,
  manifest,
  types,
}: {
  project: string;
  space: string;
  manifest: Manifest;
  types: string[];
}): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const held = rulesOf(manifest);
  // The space's default row comes last, under every type it covers (DM-97).
  const shown = [
    ...[...new Set([...types, ...held.map((rule) => rule.entityType)])].filter((type) => type !== ANY_TYPE).sort(),
    ANY_TYPE,
  ];
  const typed = shown.length > 1;
  const [draft, setDraft] = useState<Record<string, Draft> | null>(null);
  const [counts, setCounts] = useState<Count[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [proposed, setProposed] = useState(false);

  const deleted = useQuery({
    queryKey: ["projects", project, "spaces", space, "retention", "deleted"],
    retry: false,
    queryFn: async () =>
      (
        await unwrap(
          await api.GET("/api/v1/projects/{project}/spaces/{space}/retention/deleted", {
            params: { path: { project, space } },
          }),
        )
      ).counts,
  });

  const said = (error: unknown) =>
    error instanceof ApiError ? (error.problem?.detail ?? error.message) : String(error);

  const propose = useMutation({
    mutationFn: async (rules: RetentionRule[]) => {
      const { status: _status, ...envelope } = manifest as Manifest & { status?: unknown };
      const body = {
        ...envelope,
        spec: { ...(manifest.spec as object), retention: rules },
      } as unknown as ResourceProposal;
      return proposeChecked(project, "spaces", body, false);
    },
    onSuccess: () => {
      setDraft(null);
      setCounts(null);
      setProposed(true);
      void queryClient.invalidateQueries({ queryKey: ["projects", project] });
    },
    onError: (error) => setProblem(said(error)),
  });

  const preview = useMutation({
    mutationFn: async (rules: RetentionRule[]) =>
      (
        await unwrap(
          await api.POST("/api/v1/projects/{project}/spaces/{space}/retention/preview", {
            params: { path: { project, space } },
            body: { retention: rules as unknown as Record<string, unknown>[] },
          }),
        )
      ).counts,
    onSuccess: setCounts,
    onError: (error) => setProblem(said(error)),
  });

  const ruleOf = (type: string) => held.find((rule) => rule.entityType === type);
  const named = (type: string) => (type === ANY_TYPE ? t("spaces.retention.anyType") : type);
  // What an absent history falls to: the space's default, then the platform's (DM-97).
  const fallback = (type: string) =>
    type !== ANY_TYPE && ruleOf(ANY_TYPE)?.history !== undefined
      ? t("spaces.retention.spaceDefault")
      : t("spaces.retention.platformDefault");
  // Each cell says what holds for the type and which layer set it (DM-83, DM-97, T-4822).
  const plain = (type: string, kind: "current" | "history") => {
    if (kind === "current" && type === ANY_TYPE) return t("spaces.retention.perType");
    const rule: Effective = kind === "current" ? effectiveCurrent(type, held) : effectiveHistory(type, held);
    const keep = rule.value === undefined ? undefined : fromIso(rule.value);
    const value =
      rule.value === undefined || rule.value === HISTORY_FOREVER
        ? t("spaces.retention.forever")
        : rule.value === HISTORY_OFF
          ? t("spaces.retention.offShown")
          : keep === undefined
            ? rule.value
            : t(`spaces.retention.amount.${keep.unit}`, { count: keep.amount });
    return (
      <span className="flex flex-col">
        <span>{value}</span>
        <span className="text-caption text-fg-muted">{t(`spaces.retention.layer.${rule.layer}`)}</span>
      </span>
    );
  };
  // A heartbeat in words, "a row at least every 30 days", never the stored P30D (T-6089).
  const beatShown = (beat: string) => {
    const keep = fromIso(beat);
    const every = keep === undefined ? beat : t(`spaces.retention.amount.${keep.unit}`, { count: keep.amount });
    return t(`pipelines.history.heartbeat.${beat}`, {
      defaultValue: t("spaces.retention.record.atLeastEvery", { every }),
    });
  };
  // Which rows the broker writes of a type that keeps history (DM-108, DM-109, DM-132).
  const recordShown = (type: string) => {
    if (type === ANY_TYPE) return t("spaces.retention.perType");
    if (effectiveHistory(type, held).value === HISTORY_OFF) return t("spaces.retention.record.noRows");
    const rule = ruleOf(type);
    const mode = rule?.record ?? RECORD_DEFAULT;
    return (
      <span className="flex flex-col">
        <span>{t(`spaces.retention.record.mode.${mode}`, { defaultValue: mode })}</span>
        {mode === RECORD_ON_CHANGE && rule?.heartbeat ? (
          <span className="text-caption text-fg-muted">{beatShown(rule.heartbeat)}</span>
        ) : null}
        <span className="text-caption text-fg-muted">
          {rule?.record === undefined ? t("spaces.retention.layer.platform") : t("spaces.retention.layer.type")}
        </span>
      </span>
    );
  };
  const removed = (...types: string[]) => {
    const rows = (deleted.data ?? []).filter((row) => types.includes(row.entityType));
    const of = (kind: string) => rows.filter((row) => row.kind === kind).reduce((sum, row) => sum + row.count, 0);
    return t("spaces.retention.removed", { current: of("entity"), history: of("history") });
  };

  const drafted = (): RetentionRule[] | null => {
    if (draft === null) return null;
    const rules: RetentionRule[] = [];
    for (const type of shown) {
      const current = isoOf(draft[type].current);
      const history = isoOf(draft[type].history);
      if (current === null || history === null) return null;
      const own = ruleOf(type);
      const rule = { ...own, entityType: type, current, history, reason: own?.reason };
      // The space's default row keeps its recording as stored; a type writes the chosen one, and
      // the default stays unwritten where the rule never named it.
      const chosen = draft[type].record;
      const record = chosen === RECORD_DEFAULT && own?.record === undefined ? undefined : chosen;
      rules.push(type === ANY_TYPE ? rule : withRecord(rule, record, draft[type].heartbeat));
    }
    return written(rules);
  };

  const save = () => {
    setProblem(null);
    const rules = drafted();
    if (rules === null) {
      setProblem(t("spaces.retention.invalid"));
      return;
    }
    if (shortens(held, rules)) {
      preview.mutate(rules);
    } else {
      propose.mutate(rules);
    }
  };

  const edit = () => {
    setProposed(false);
    setDraft(
      Object.fromEntries(
        shown.map((type) => [
          type,
          {
            current: cellOf(ruleOf(type)?.current),
            history: cellOf(ruleOf(type)?.history),
            record: ruleOf(type)?.record ?? RECORD_DEFAULT,
            heartbeat: ruleOf(type)?.heartbeat ?? "PT1H",
          },
        ]),
      ),
    );
  };

  const editor = (type: string, kind: "current" | "history") => {
    // The space's default sets history only; current state is removed per type (DM-97).
    if (draft === null || (kind === "current" && type === ANY_TYPE)) return null;
    const cell = draft[type][kind];
    const set = (next: Partial<Cell>) =>
      setDraft({ ...draft, [type]: { ...draft[type], [kind]: { ...cell, ...next } } });
    const label = t(`spaces.retention.${kind}`);
    return (
      <span className="flex gap-2">
        {cell.unit !== "forever" && cell.unit !== "off" && (
          <Input
            type="number"
            min={1}
            className="w-20"
            aria-label={t("spaces.retention.amountOf", { type: named(type), what: label })}
            value={cell.amount}
            onChange={(event) => set({ amount: event.target.value })}
          />
        )}
        <Select
          aria-label={t("spaces.retention.unitOf", { type: named(type), what: label })}
          value={cell.unit}
          onChange={(event) => set({ unit: event.target.value as Cell["unit"], amount: cell.amount || "1" })}
        >
          {/* History is Off or kept for a while; forever stays only where it already is (T-4275). */}
          {kind === "current" ? (
            <option value="forever">{t("spaces.retention.forever")}</option>
          ) : (
            <>
              {(cell.stored === undefined || cell.stored === HISTORY_FOREVER) && (
                <option value="forever">
                  {cell.stored === HISTORY_FOREVER
                    ? t("spaces.retention.foreverAsNow")
                    : t("spaces.retention.defaultAsNow", { fallback: fallback(type) })}
                </option>
              )}
              <option value="off">{t("spaces.retention.off")}</option>
            </>
          )}
          {UNITS.map((unit) => (
            <option key={unit} value={unit}>
              {t(`spaces.retention.unit.${unit}`)}
            </option>
          ))}
        </Select>
      </span>
    );
  };

  const recordEditor = (type: string) => {
    if (draft === null || type === ANY_TYPE) return null;
    const row = draft[type];
    const set = (next: Partial<Draft>) => setDraft({ ...draft, [type]: { ...row, ...next } });
    // A stored value the editor does not offer (as `never`) stays a choice while it is held.
    const modes: readonly string[] = (RECORDS as readonly string[]).includes(row.record) ? RECORDS : [...RECORDS, row.record];
    const beats: readonly string[] = (HEARTBEATS as readonly string[]).includes(row.heartbeat) ? HEARTBEATS : [...HEARTBEATS, row.heartbeat];
    return (
      <span className="flex flex-col gap-1">
        <Select
          className="min-w-60"
          aria-label={t("spaces.retention.record.of", { type: named(type) })}
          aria-describedby="retention-record-hint"
          value={row.record}
          onChange={(event) => set({ record: event.target.value })}
        >
          {modes.map((mode) => (
            <option key={mode} value={mode}>
              {mode === RECORD_DEFAULT
                ? t("spaces.retention.record.asDefault", { mode: t(`spaces.retention.record.mode.${mode}`) })
                : t(`spaces.retention.record.mode.${mode}`, { defaultValue: mode })}
            </option>
          ))}
        </Select>
        {row.record === RECORD_ON_CHANGE ? (
          <Select
            className="min-w-60"
            aria-label={t("spaces.retention.record.heartbeatOf", { type: named(type) })}
            aria-describedby="retention-record-hint"
            value={row.heartbeat}
            onChange={(event) => set({ heartbeat: event.target.value })}
          >
            {beats.map((beat) => (
              <option key={beat} value={beat}>
                {beatShown(beat)}
              </option>
            ))}
          </Select>
        ) : null}
      </span>
    );
  };

  const pending = drafted();
  // Outside the editor, types under one identical rule share a row, so the rules that differ
  // stand out (T-6089); the space's default row stays on its own, last.
  const sameRule = (type: string) => {
    const rule = ruleOf(type);
    return JSON.stringify([
      effectiveCurrent(type, held),
      effectiveHistory(type, held),
      rule?.record,
      rule?.record === RECORD_ON_CHANGE ? rule.heartbeat : undefined,
    ]);
  };
  const rows: string[][] = [];
  for (const type of shown) {
    const same = draft === null && type !== ANY_TYPE ? rows.find((row) => row[0] !== ANY_TYPE && sameRule(row[0]) === sameRule(type)) : undefined;
    if (same) same.push(type);
    else rows.push([type]);
  }
  const typesCell = (types: string[]) =>
    types.length === 1 ? (
      named(types[0])
    ) : (
      <details>
        <summary className="cursor-pointer">{t("spaces.retention.sameRule", { count: types.length })}</summary>
        <span className="text-caption text-fg-muted">{types.join(", ")}</span>
      </details>
    );
  return (
    <div className="space-y-3">
      <p className="text-body text-fg-muted">{t("spaces.retention.lead")}</p>
      <p className="text-body text-fg-muted">
        {t("spaces.retention.entityWins")} <RetentionGuide section="which-one-wins" />
      </p>
      {!typed && <p className="text-body text-fg-muted">{t("spaces.retention.noTypes")}</p>}
      <Table caption={t("spaces.retention.caption")}>
        <TableHead>
          <TableHeaderCell>{t("spaces.retention.type")}</TableHeaderCell>
          <TableHeaderCell>{t("spaces.retention.current")}</TableHeaderCell>
          <TableHeaderCell>{t("spaces.retention.history")}</TableHeaderCell>
          <TableHeaderCell>{t("spaces.retention.record.column")}</TableHeaderCell>
          <TableHeaderCell>{t("spaces.retention.deleted")}</TableHeaderCell>
        </TableHead>
        <TableBody>
          {rows.map((types) => [types[0], types] as const).map(([type, types]) => (
            <TableRow key={type}>
              <TableCell>{typesCell(types)}</TableCell>
              <TableCell>{editor(type, "current") ?? plain(type, "current")}</TableCell>
              <TableCell>{editor(type, "history") ?? plain(type, "history")}</TableCell>
              <TableCell>{recordEditor(type) ?? recordShown(type)}</TableCell>
              <TableCell>{deleted.isError ? t("spaces.retention.deletedUnknown") : removed(...types)}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <p className="text-caption text-fg-muted">{t("spaces.retention.deletedSince")}</p>
      {draft !== null && (
        <div className="space-y-1 text-caption text-fg-muted">
          <p>{t("spaces.retention.durationHint")}</p>
          <p id="retention-record-hint">{t("spaces.retention.record.hint")}</p>
        </div>
      )}
      {problem !== null && <Alert tone="danger">{problem}</Alert>}
      {proposed && <Alert tone="success">{t("spaces.retention.proposed")}</Alert>}
      {draft === null ? (
        <PermissionGuard project={project} kind="ContextSpace" verb="approve">
          <Button variant="secondary" onClick={edit}>
            {t("spaces.retention.edit")}
          </Button>
        </PermissionGuard>
      ) : (
        <span className="flex gap-2">
          <Button onClick={save} loading={preview.isPending || propose.isPending}>
            {t("spaces.retention.save")}
          </Button>
          <Button
            variant="secondary"
            onClick={() => {
              setDraft(null);
              setProblem(null);
            }}
          >
            {t("spaces.retention.cancel")}
          </Button>
        </span>
      )}
      <ConfirmDialog
        open={counts !== null}
        onOpenChange={(open) => {
          if (!open) setCounts(null);
        }}
        title={t("spaces.retention.confirmTitle", { space })}
        description={t("spaces.retention.confirmLead")}
        confirmLabel={t("spaces.retention.confirm")}
        pending={propose.isPending}
        onConfirm={() => {
          if (pending !== null) propose.mutate(pending);
        }}
      >
        {counts !== null && counts.length === 0 ? (
          <p className="text-body">{t("spaces.retention.nothingNow")}</p>
        ) : (
          <ul className="list-disc pl-5 text-body">
            {(counts ?? []).map((row) => (
              <li key={`${row.entityType}-${row.kind}`}>
                {t(`spaces.retention.wouldDelete.${row.kind === "entity" ? "entity" : "history"}`, {
                  type: named(row.entityType),
                  count: row.count,
                })}
              </li>
            ))}
          </ul>
        )}
      </ConfirmDialog>
    </div>
  );
}
