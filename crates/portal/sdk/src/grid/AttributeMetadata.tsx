/**
 * Who changed one attribute, when and through what (T-5529, DM-114): Created and Last changed from
 * the entity's `sysAttrs`, and the writer's name, kind, organization, channel and reason from the
 * newest version's provenance, read once as `versions=all&lastN=1` through the grid's own source.
 *
 * The read is the person's own session through the same Endpoint, so the panel never learns more
 * than the source may: a 403 on the versions read is shown as "hidden", never retried as anyone
 * else, and nothing is kept once the panel closes. A field the server did not return says why it
 * is missing, so an empty value is never mistaken for "no data".
 */
import React, { useEffect, useState } from "react";
import type { RichCell } from "./model";
import { SourceError } from "./source";
import type { EntitySource, Provenance } from "./source";

/** Why a field is not shown: the role has no `readVersions`, the Endpoint withholds the member, or it is a public view. */
export type HiddenReason = "history" | "member" | "public";

export interface MetadataLabels {
  created: string;
  modified: string;
  changedBy: string;
  organization: string;
  channel: string;
  reason: string;
  /** "hidden: no access", followed by the reason in brackets. */
  hidden: string;
  hiddenWhy: Record<HiddenReason, string>;
  /** No version of this attribute was recorded: a real absence, said in words. */
  notRecorded: string;
  /** The write carried no `JC-Reason`: a reason is optional, so this is not a hidden one. */
  noReason: string;
  loading: string;
  error: string;
  actorKind: Record<string, string>;
  channels: Record<string, string>;
}

export const DEFAULT_METADATA_LABELS: MetadataLabels = {
  created: "Created",
  modified: "Last changed",
  changedBy: "Changed by",
  organization: "Organization",
  channel: "Through",
  reason: "Reason",
  hidden: "hidden: no access",
  hiddenWhy: {
    history: "history not granted",
    member: "this view does not pass it",
    public: "public view",
  },
  notRecorded: "No change was recorded",
  noReason: "No reason was given",
  loading: "Loading…",
  error: "The change record could not be read",
  actorKind: { person: "a person", pipeline: "a pipeline", agent: "an AI agent", serviceAccount: "a service account" },
  channels: { ui: "Portal", api: "API", pipeline: "Pipeline", mqtt: "MQTT pipeline", assistant: "Assistant" },
};

const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 365 * 24 * 3600],
  ["month", 30 * 24 * 3600],
  ["day", 24 * 3600],
  ["hour", 3600],
  ["minute", 60],
  ["second", 1],
];

/** A timestamp as the person reads it: "3 hours ago" and the full date in their time zone. */
export function when(iso: string, locale: string | undefined, now: Date, timeZone?: string): { relative: string; absolute: string } | null {
  const at = Date.parse(iso);
  if (!Number.isFinite(at)) return null;
  const seconds = Math.round((at - now.getTime()) / 1000);
  const [unit, size] = UNITS.find(([, size]) => Math.abs(seconds) >= size) ?? ["second", 1];
  return {
    relative: new Intl.RelativeTimeFormat(locale, { numeric: "auto" }).format(Math.round(seconds / size), unit),
    absolute: new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short", timeZone }).format(at),
  };
}

/** The newest version's provenance, the one written last so a correction wins over what it replaced; `{}` for a version without any. */
export function newest(points: { at: string; provenance?: Provenance }[]): Provenance | undefined {
  if (points.length === 0) return undefined;
  const stamp = (point: { at: string; provenance?: Provenance }) => Date.parse(point.provenance?.writtenAt ?? point.at) || 0;
  return [...points].sort((a, b) => stamp(b) - stamp(a))[0].provenance ?? {};
}

export interface AttributeMetadataProps {
  source: Pick<EntitySource, "history">;
  id: string;
  attr: string;
  /** The attribute as the entity read it with `sysAttrs`; its first instance where it has several. */
  cell: RichCell | RichCell[] | undefined;
  /** A public Endpoint never passes who wrote a value (DM-139/140): nothing is asked for. */
  publicView?: boolean;
  locale?: string;
  timeZone?: string;
  labels?: Partial<MetadataLabels>;
  now?: () => Date;
}

type Read = { state: "loading" } | { state: "done"; provenance?: Provenance } | { state: "hidden"; why: HiddenReason } | { state: "error"; message?: string };

export function AttributeMetadata(props: AttributeMetadataProps): React.JSX.Element {
  const { source, id, attr, publicView, locale, timeZone } = props;
  const labels: MetadataLabels = { ...DEFAULT_METADATA_LABELS, ...props.labels };
  const cell = Array.isArray(props.cell) ? props.cell[0] : props.cell;
  const [read, setRead] = useState<Read>({ state: "loading" });

  useEffect(() => {
    if (publicView) return setRead({ state: "hidden", why: "public" });
    if (!source.history) return setRead({ state: "hidden", why: "member" });
    let live = true;
    setRead({ state: "loading" });
    source.history(id, attr, { versions: true, lastN: 1 }).then(
      (points) => live && setRead({ state: "done", provenance: newest(points) }),
      (err: unknown) => {
        if (!live) return;
        if (err instanceof SourceError && err.status === 403) setRead({ state: "hidden", why: "history" });
        else setRead({ state: "error", message: err instanceof Error && err.message ? err.message : undefined });
      },
    );
    return () => {
      live = false;
    };
  }, [source, id, attr, publicView]);

  const hidden = (why: HiddenReason) => <span className="jc-grid-meta-hidden">{`${labels.hidden} (${labels.hiddenWhy[why]})`}</span>;
  const time = (iso: string | undefined) => {
    const shown = iso ? when(iso, locale, props.now?.() ?? new Date(), timeZone) : null;
    if (!iso || !shown) return hidden(publicView ? "public" : "member");
    return (
      <time dateTime={iso} title={shown.absolute}>
        {shown.relative}
        <span className="jc-grid-meta-absolute">{shown.absolute}</span>
      </time>
    );
  };
  const written = (pick: (p: Provenance) => string | undefined): React.ReactNode => {
    if (read.state === "loading") return <span aria-busy="true">{labels.loading}</span>;
    if (read.state === "hidden") return hidden(read.why);
    if (read.state === "error") return <span className="jc-grid-meta-error">{read.message ?? labels.error}</span>;
    if (!read.provenance) return labels.notRecorded;
    return pick(read.provenance) ?? hidden("member");
  };
  const kindOf = (p: Provenance) => (p.actorKind ? labels.actorKind[p.actorKind] ?? p.actorKind : undefined);
  const rows: [string, React.ReactNode][] = [
    [labels.created, time(cell?.createdAt)],
    [labels.modified, time(cell?.modifiedAt)],
  ];
  // One refusal covers who, where from, through what and why: said once, not four times.
  if (read.state === "hidden") rows.push([labels.changedBy, hidden(read.why)]);
  // Nothing recorded is said once too (UI judge, T-6344).
  else if (read.state === "done" && !read.provenance) rows.push([labels.changedBy, labels.notRecorded]);
  else rows.push(
    [labels.changedBy, written((p) => (p.actor && kindOf(p) ? `${p.actor} (${kindOf(p)})` : p.actor ?? kindOf(p)))],
    [labels.organization, written((p) => p.organization)],
    [labels.channel, written((p) => (p.channel ? labels.channels[p.channel] ?? p.channel : undefined))],
    [labels.reason, written((p) => p.reason ?? (Object.keys(p).length > 0 ? labels.noReason : undefined))],
  );
  return (
    <dl className="jc-grid-detail-list jc-grid-meta">
      {rows.map(([term, value]) => (
        <div key={term} className="jc-grid-detail-item">
          <dt>{term}</dt>
          <dd>{value}</dd>
        </div>
      ))}
    </dl>
  );
}
