/**
 * Two steps of the pipeline workbench (T-4842): Target, the type the pipeline writes as the
 * space's model states it and as its Endpoint lets the pipeline write it, and the Mapped output
 * as each record's attributes, each with its verdict and end. The writability is the server's
 * own judgement (jc_pipeline_validate `type`, the gateway's grant and the broker's view); this
 * page decides nothing itself.
 */
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { callOperation } from "../../api/operations";
import { Alert, Badge, Button, Select, Table, TableBody, TableCell, TableHead, TableHeaderCell, TableRow } from "../../components/ui";
import { RECORD_LINK_STYLE } from "../../components/RecordLink";
import { attributesOf } from "./OutputMapper";
import type { Attribute } from "./outputMapper";
import { HEARTBEATS, KEEPS, RECEIVED, planOf, withPlan, type History, type Keep, type Plan } from "./history";

/** One attribute as the target Endpoint lets the pipeline write it. */
export interface Writable {
  name: string;
  writable: boolean;
  reason?: string;
}

/** The members of an entity that are no attribute a mapping maps (NGSI-LD system members, DM-130). */
const SYSTEM = new Set(["id", "type", "@context", "scope", "createdAt", "modifiedAt", "deletedAt", "expiresAt"]);

export function readWritable(output: unknown): Writable[] {
  const list = (output as { writable?: unknown } | null)?.writable;
  return Array.isArray(list)
    ? list
        .filter((one): one is Record<string, unknown> => typeof one === "object" && one !== null && typeof one.name === "string")
        .map((one) => ({
          name: String(one.name),
          writable: one.writable === true,
          ...(typeof one.reason === "string" ? { reason: one.reason } : {}),
        }))
    : [];
}

export interface TargetType {
  model?: { name: string; version: string };
  attributes: Attribute[];
  /** Absent until the server answered, or when it could not judge. */
  writable?: Writable[];
  loading: boolean;
  error?: string;
}

/** The type the pipeline writes, from the space's model and the server's write judgement. */
export function useTargetType(project: string, space: string | undefined, type: string | undefined, manifest: unknown, endpoint: string | undefined): TargetType {
  const typed = useQuery({
    queryKey: ["type-attributes", project, space, type],
    enabled: Boolean(space && type),
    queryFn: () => attributesOf(project, space ?? "", type ?? ""),
    retry: false,
  });
  const judged = useQuery({
    // The grants follow the Endpoint and the type; a mapping edit changes neither.
    queryKey: ["pipeline-workbench", project, "writable", endpoint, type],
    enabled: Boolean(space && type && endpoint && typed.data),
    retry: false,
    refetchOnWindowFocus: false,
    queryFn: async () => {
      const answer = await callOperation(project, "jc_pipeline_validate", { pipeline: manifest, records: [], type });
      if (!answer.ok) throw new Error(answer.reason ?? "");
      return readWritable(answer.output);
    },
  });
  const error = typed.error ?? judged.error;
  return {
    ...(typed.data ? { model: { name: typed.data.model, version: typed.data.version } } : {}),
    attributes: typed.data?.attributes ?? [],
    ...(judged.data ? { writable: judged.data } : {}),
    loading: typed.isFetching || judged.isFetching,
    ...(error ? { error: error instanceof Error ? error.message : String(error) } : {}),
  };
}

/** The attributes a lifetime may name: the type's own, the Endpoint granting each. */
export function grantedNames(target: TargetType): string[] {
  const refused = new Set((target.writable ?? []).filter((one) => !one.writable).map((one) => one.name));
  return target.attributes.map((a) => a.name).filter((name) => !SYSTEM.has(name) && !refused.has(name));
}

/** A slot's value type in words: the coded values, the type and its unit. */
function valueOf(attribute: Attribute): string {
  if (attribute.values?.length) return attribute.values.slice(0, 6).join(" · ") + (attribute.values.length > 6 ? " …" : "");
  if (attribute.relationship) return `→ ${attribute.relationship.target}`;
  const { minimum: low, maximum: high } = attribute;
  const range = low != null && high != null ? ` ${low}–${high}` : low != null ? ` ≥ ${low}` : high != null ? ` ≤ ${high}` : "";
  const unit = attribute.unit?.code ?? attribute.unit?.ucum;
  return `${attribute.format ?? attribute.valueType ?? "any"}${range}${unit ? `, ${unit}` : ""}`;
}

export interface TargetCardProps {
  space: string;
  type: string | undefined;
  target: TargetType;
  /** `spec.history` of the draft; with `onHistory`, each attribute gets its History column. */
  history?: History[];
  onHistory?: (history: History[] | undefined) => void;
  /** The date-time slots a row's time may come from, picked from the sample. */
  timeSlots?: string[];
  /** The record check's history findings of the draft (PL-109); absent until it ran. */
  historyFindings?: CheckFinding[];
}

interface HistoryCellProps {
  name: string;
  plan: Plan | undefined;
  timeSlots: string[];
  findings: CheckFinding[] | undefined;
  onPlan: (plan: Plan | undefined) => void;
}

/** One attribute's history plan: what is kept, the time of a row, and the server's verdict on it. */
function HistoryCell({ name, plan, timeSlots, findings, onPlan }: HistoryCellProps): JSX.Element {
  const { t } = useTranslation();
  const id = `workbench-history-${name}`;
  const refused = findings?.filter((f) => f.field === name) ?? [];
  const times = [...new Set([...timeSlots, ...(plan?.observedAt && plan.observedAt !== RECEIVED ? [plan.observedAt] : [])])];
  return (
    <div className="flex min-w-48 flex-col gap-1">
      <Select
        id={id}
        aria-label={t("pipelines.history.keepOf", { name })}
        value={plan?.keep ?? ""}
        onChange={(event) => onPlan(event.target.value ? { ...(plan ?? {}), keep: event.target.value as Keep } : undefined)}
      >
        <option value="">{t("pipelines.history.asMapped")}</option>
        {KEEPS.map((keep) => (
          <option key={keep} value={keep}>
            {t(`pipelines.history.keep.${keep}`)}
          </option>
        ))}
      </Select>
      {plan ? (
        <Select
          id={`${id}-time`}
          aria-label={t("pipelines.history.timeOf", { name })}
          value={plan.observedAt ?? ""}
          onChange={(event) => onPlan({ ...plan, observedAt: event.target.value || undefined })}
        >
          <option value="">{t("pipelines.history.timeMapped")}</option>
          {times.map((slot) => (
            <option key={slot} value={slot}>
              {t("pipelines.history.timeFrom", { slot })}
            </option>
          ))}
          <option value={RECEIVED}>{t("pipelines.history.timeReceived")}</option>
        </Select>
      ) : null}
      {plan?.keep === "changes" ? (
        <Select
          id={`${id}-heartbeat`}
          aria-label={t("pipelines.history.heartbeatOf", { name })}
          value={plan.heartbeat ?? "PT1H"}
          onChange={(event) => onPlan({ ...plan, heartbeat: event.target.value })}
        >
          {HEARTBEATS.map((beat) => (
            <option key={beat} value={beat}>
              {t(`pipelines.history.heartbeat.${beat}`)}
            </option>
          ))}
        </Select>
      ) : null}
      {refused.length > 0 ? (
        refused.map((finding) => (
          <span key={finding.rule} className="text-caption" role="status">
            <Badge tone="danger">{t("pipelines.history.refused")}</Badge> <span className="text-fg-muted">{finding.reason}</span>
          </span>
        ))
      ) : plan && findings ? (
        <span className="text-caption" role="status">
          <Badge tone="success">{t("pipelines.history.kept")}</Badge>
        </span>
      ) : null}
    </div>
  );
}

/** Step 3's card: space, model version, the type written and, per attribute, whether it is writable here. */
export function TargetCard({ space, type, target, history, onHistory, timeSlots = [], historyFindings }: TargetCardProps): JSX.Element {
  const { t } = useTranslation();
  const judged = (name: string) => target.writable?.find((one) => one.name === name);
  return (
    <section aria-label={t("pipelines.workbench.target.card")} className="flex flex-col gap-2 rounded border border-border p-2" data-testid="workbench-target-card">
      <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-caption">
        <dt className="text-fg-muted">{t("pipelines.workbench.target.space")}</dt>
        <dd className="font-medium text-fg">{space}</dd>
        <dt className="text-fg-muted">{t("pipelines.workbench.target.model")}</dt>
        <dd className="font-medium text-fg">{target.model ? `${target.model.name} ${target.model.version}` : "—"}</dd>
        <dt className="text-fg-muted">{t("pipelines.workbench.target.type")}</dt>
        <dd className="font-medium text-fg">{type ?? t("pipelines.workbench.target.noType")}</dd>
      </dl>
      {target.error ? (
        <Alert role="alert" tone="danger">
          {target.error}
        </Alert>
      ) : null}
      {type && target.attributes.length > 0 ? (
        <>
          <p className="text-caption text-fg-muted">{t("pipelines.workbench.target.explain", { type })}</p>
          {onHistory ? <p className="text-caption text-fg-muted">{t("pipelines.history.explain")}</p> : null}
          <Table caption={t("pipelines.workbench.target.caption", { type })} {...(onHistory ? {} : { maxHeight: "max-h-80" })}>
            <TableHead>
              <TableHeaderCell>{t("pipelines.workbench.target.field")}</TableHeaderCell>
              <TableHeaderCell>{t("pipelines.workbench.target.kind")}</TableHeaderCell>
              <TableHeaderCell>{t("pipelines.workbench.target.value")}</TableHeaderCell>
              <TableHeaderCell>{t("pipelines.workbench.target.required")}</TableHeaderCell>
              <TableHeaderCell>{t("pipelines.workbench.target.writable")}</TableHeaderCell>
              {onHistory ? <TableHeaderCell>{t("pipelines.history.column")}</TableHeaderCell> : null}
            </TableHead>
            <TableBody>
              {target.attributes
                .filter((a) => !SYSTEM.has(a.name))
                .map((attribute) => {
                  const verdict = judged(attribute.name);
                  return (
                    <TableRow key={attribute.name}>
                      <TableCell>
                        <span className="font-mono">{attribute.name}</span>
                      </TableCell>
                      <TableCell>{attribute.kind}</TableCell>
                      <TableCell>{valueOf(attribute)}</TableCell>
                      <TableCell>{attribute.required ? t("pipelines.workbench.target.isRequired") : t("pipelines.workbench.target.optional")}</TableCell>
                      <TableCell>
                        {verdict === undefined ? (
                          <span className="text-fg-subtle">{target.loading ? t("pipelines.workbench.target.judging") : "—"}</span>
                        ) : verdict.writable ? (
                          <span>
                            <Badge tone="success">{t("pipelines.workbench.target.isWritable")}</Badge>
                            {verdict.reason ? <span className="text-fg-muted"> {verdict.reason}</span> : null}
                          </span>
                        ) : (
                          <span>
                            <Badge tone="danger">
                              <span aria-hidden="true">🔒 </span>
                              {t("pipelines.workbench.target.locked")}
                            </Badge>{" "}
                            <span className="text-fg-muted">{verdict.reason}</span>
                          </span>
                        )}
                      </TableCell>
                      {onHistory && type ? (
                        <TableCell>
                          {verdict?.writable === false ? (
                            <span className="text-fg-subtle">—</span>
                          ) : (
                            <HistoryCell
                              name={attribute.name}
                              plan={planOf(history, type, attribute.name)}
                              timeSlots={timeSlots.filter((slot) => slot !== attribute.name)}
                              findings={historyFindings}
                              onPlan={(plan) => onHistory(withPlan(history, type, attribute.name, plan))}
                            />
                          )}
                        </TableCell>
                      ) : null}
                    </TableRow>
                  );
                })}
              <TableRow>
                <TableCell>
                  <span className="font-mono">expiresAt</span>
                </TableCell>
                <TableCell>{t("pipelines.workbench.target.lifetimeKind")}</TableCell>
                <TableCell>{t("pipelines.workbench.target.instant")}</TableCell>
                <TableCell>{t("pipelines.workbench.target.optional")}</TableCell>
                <TableCell>
                  <a className={RECORD_LINK_STYLE} href="#workbench-lifetime">
                    {t("pipelines.workbench.target.inLifetime")}
                  </a>
                </TableCell>
                {onHistory ? <TableCell>—</TableCell> : null}
              </TableRow>
            </TableBody>
          </Table>
        </>
      ) : null}
    </section>
  );
}

/** One finding of a record's check, as step 5 and 7 read it. */
export interface CheckFinding {
  source: string;
  rule: string;
  field: string;
  reason: string;
  /** The access request that would cure a missing grant on another space's Endpoint (PL-90). */
  access?: FindingAccess;
}

/** The Endpoint, operation and type a missing grant names (T-5369). */
export interface FindingAccess {
  project: string;
  name: string;
  operations: "read" | "write";
  type?: string;
}

/**
 * What a finding's "Request access" does: `where` names the space the server offers the Endpoint
 * under (else `project/name`), `open` opens the prefilled form, `disabledReason` says why it cannot
 * open yet.
 */
export interface AccessAsk {
  where: (access: FindingAccess) => string;
  open: (access: FindingAccess) => void;
  disabledReason?: string;
}

/** An attribute's value in a few words: the value and unit, a point, the object, the languages. */
export function valueText(member: unknown): string {
  if (typeof member !== "object" || member === null || Array.isArray(member)) return JSON.stringify(member) ?? "";
  const attribute = member as Record<string, unknown>;
  if (attribute.type === "Relationship") return String(attribute.object ?? "");
  if (attribute.type === "LanguageProperty" && typeof attribute.languageMap === "object" && attribute.languageMap !== null) {
    return Object.entries(attribute.languageMap as Record<string, unknown>)
      .map(([lang, text]) => `${lang}: ${String(text)}`)
      .join(" · ");
  }
  const value = attribute.value;
  if (typeof value === "object" && value !== null && (value as { type?: unknown }).type === "Point") {
    const [lon, lat] = ((value as { coordinates?: unknown[] }).coordinates ?? []).map(Number);
    return `Point ${lon}, ${lat}`;
  }
  const text = typeof value === "string" ? `"${value}"` : (JSON.stringify(value) ?? "");
  return attribute.unitCode ? `${text} ${String(attribute.unitCode)}` : text;
}

/** One finding: where it comes from, its rule, the field when it names one, and why. */
function FindingLine({ finding, named = true, ask }: { finding: CheckFinding; named?: boolean; ask?: AccessAsk }): JSX.Element {
  const { t } = useTranslation();
  const access = ask ? finding.access : undefined;
  return (
    <li className="text-caption">
      <Badge tone="danger">{t(`pipelines.workbench.validation.source.${finding.source}`)}</Badge>{" "}
      {access && ask ? (
        <>
          <span className="font-medium text-fg">{t("pipelines.access.noAccess", { where: ask.where(access) })}</span>{" "}
          <Button
            size="sm"
            variant="secondary"
            disabled={ask.disabledReason !== undefined}
            disabledReason={ask.disabledReason}
            aria-label={t("pipelines.access.requestFor", { endpoint: `${access.project}/${access.name}` })}
            onClick={() => ask.open(access)}
          >
            {t("pipelines.access.request")}
          </Button>{" "}
        </>
      ) : (
        <span className="font-mono text-fg-subtle">{finding.rule}</span>
      )}{" "}
      {named && finding.field ? <span className="font-mono">{finding.field}</span> : null} <span className="text-fg-muted">{finding.reason}</span>
    </li>
  );
}

export interface RecordTableProps {
  index: number;
  record: Record<string, unknown>;
  verdict?: { ok: boolean; findings: CheckFinding[]; expires?: { entity?: string; attributes: Record<string, string> } };
  /** Turns a missing grant into "No access to <space>: Request access" (T-5369). */
  ask?: AccessAsk;
}

/** Step 5: one record as it would be written, attribute by attribute, each with its verdict and end. */
export function RecordTable({ index, record, verdict, ask }: RecordTableProps): JSX.Element {
  const { t } = useTranslation();
  const names = Object.keys(record).filter((name) => !["id", "type", "@context"].includes(name));
  const of = (name: string) => (verdict?.findings ?? []).filter((f) => f.field === name || f.field.startsWith(`${name}.`) || f.field.startsWith(`${name}/`) || f.field.startsWith(`/${name}`));
  const placed = new Set(names.flatMap((name) => of(name)));
  const loose = (verdict?.findings ?? []).filter((f) => !placed.has(f));
  const id = typeof record.id === "string" ? record.id : t("pipelines.workbench.output.recordN", { n: index + 1 });
  const problems = verdict?.findings.length ?? 0;
  return (
    <section aria-label={id} className="flex flex-col gap-1 rounded border border-border p-2" data-testid={`workbench-record-${index}`}>
      <div className="flex flex-wrap items-center gap-2">
        <span className="min-w-0 font-mono text-caption text-fg [overflow-wrap:anywhere]">{id}</span>
        {verdict === undefined ? null : verdict.ok ? (
          <Badge tone="success">{t("pipelines.workbench.validation.valid")}</Badge>
        ) : (
          <Badge tone="danger">{t("pipelines.workbench.output.problemCount", { count: problems })}</Badge>
        )}
      </div>
      {loose.length > 0 ? (
        <ul className="flex flex-col gap-0.5">
          {loose.map((finding, at) => (
            <FindingLine key={at} finding={finding} ask={ask} />
          ))}
        </ul>
      ) : null}
      <Table caption={t("pipelines.workbench.output.recordCaption", { id })}>
        <TableHead>
          <TableHeaderCell>{t("pipelines.workbench.target.field")}</TableHeaderCell>
          <TableHeaderCell>{t("pipelines.workbench.output.value")}</TableHeaderCell>
          <TableHeaderCell>{t("pipelines.workbench.output.check")}</TableHeaderCell>
          <TableHeaderCell>{t("pipelines.workbench.output.expires")}</TableHeaderCell>
        </TableHead>
        <TableBody>
          {names.map((name) => {
            const found = of(name);
            const member = record[name];
            const observed = typeof member === "object" && member !== null ? (member as { observedAt?: unknown }).observedAt : undefined;
            return (
              <TableRow key={name}>
                <TableCell>
                  <span className="font-mono">{name}</span>
                </TableCell>
                <TableCell>
                  <span className="font-mono text-caption [overflow-wrap:anywhere]">{valueText(member)}</span>
                  {typeof observed === "string" ? <span className="block text-caption text-fg-muted">{t("pipelines.workbench.output.observedAt", { at: observed })}</span> : null}
                </TableCell>
                <TableCell>
                  {verdict === undefined ? null : found.length === 0 ? (
                    <Badge tone="success">{t("pipelines.workbench.output.passes")}</Badge>
                  ) : (
                    <ul className="flex flex-col gap-0.5">
                      {found.map((finding, at) => (
                        <FindingLine key={at} finding={finding} named={false} ask={ask} />
                      ))}
                    </ul>
                  )}
                </TableCell>
                <TableCell>{verdict?.expires?.attributes[name] ?? (name === "expiresAt" ? String(member) : "—")}</TableCell>
              </TableRow>
            );
          })}
          {verdict?.expires?.entity && !names.includes("expiresAt") ? (
            <TableRow>
              <TableCell>
                <span className="font-mono">expiresAt</span>
              </TableCell>
              <TableCell>{t("pipelines.workbench.output.entityLifetime")}</TableCell>
              <TableCell>—</TableCell>
              <TableCell>{verdict.expires.entity}</TableCell>
            </TableRow>
          ) : null}
        </TableBody>
      </Table>
    </section>
  );
}
