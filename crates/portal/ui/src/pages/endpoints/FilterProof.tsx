import { useEffect, useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { api, ApiError, unwrap } from "../../api/client";
import type { Manifest } from "../../api/manifest";
import { endpointUrl } from "../../components/endpoints/links";
import {
  Alert,
  Button,
  Field,
  Select,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";
import { Tabs, tabPanelProps } from "../../components/ui/Tabs";

/** One page of the proof: what CIM 009 calls a page, and what the preview allows. */
export const PROOF_PAGE = 12;
/** How long an edit rests before the left side is asked again. */
const SETTLE_MS = 600;
const RESULTS_COUNT = "NGSILD-Results-Count";
/** The members of an entity that are its identity, not attributes a filter can hide. */
const IDENTITY = new Set(["id", "type", "@context"]);

/** An entity as the NGSI-LD surface answers it, normalized. */
export type ProofEntity = Record<string, unknown> & { id: string };

/** The filter being edited: what `POST …/preview` takes in place of the saved one (EP-85). */
export interface FilterDraft {
  classes: { name: string; slots: string[] }[];
  /** The non-empty conditions only: `q`, `scopeQ`, `geoQ`, `temporalQ`. */
  filter: Record<string, string>;
  hiddenAttributes: string[];
}

/** The projection's classes with the slots each keeps (MP-01). */
export function classesWithSlots(projection: Manifest): { name: string; slots: string[] }[] {
  const classes = (projection.spec as { classes?: Array<{ name?: string; slots?: string[] }> }).classes ?? [];
  return classes
    .filter((klass): klass is { name: string; slots?: string[] } => typeof klass.name === "string" && klass.name !== "")
    .map((klass) => ({ name: klass.name, slots: klass.slots ?? [] }));
}

/** One row of the proof, aligned on the entity id. */
export interface ProofRow {
  id: string;
  /** The entity as the endpoint would serve it under the draft; absent when the filter drops it. */
  served?: ProofEntity;
  /** The entity as the space holds it; absent when the person cannot read the space. */
  original?: ProofEntity;
  /** Attributes the space holds and the endpoint would not serve, for a row it serves. */
  struck: string[];
}

/**
 * The rows of one page, aligned on the entity id (EP-86).
 *
 * With the original page at hand, it decides the rows: the served side was asked for exactly those
 * ids, so a row missing there is a row the filter drops. Without it (a person who may not read the
 * space), the served page is the whole proof.
 */
export function alignRows(original: ProofEntity[] | undefined, served: ProofEntity[]): ProofRow[] {
  const byId = new Map(served.map((entity) => [entity.id, entity]));
  if (original === undefined) {
    return served.map((entity) => ({ id: entity.id, served: entity, struck: [] }));
  }
  return original.map((entity) => {
    const kept = byId.get(entity.id);
    return {
      id: entity.id,
      original: entity,
      served: kept,
      struck: kept === undefined ? [] : attributesOf(entity).filter((name) => !(name in kept)),
    };
  });
}

/** The attribute names of an entity, in the order it carries them. */
export function attributesOf(entity: ProofEntity): string[] {
  return Object.keys(entity).filter((name) => !IDENTITY.has(name));
}

/** The attributes struck on at least one row: what the summary counts as hidden. */
export function hiddenOf(rows: ProofRow[]): string[] {
  return [...new Set(rows.flatMap((row) => row.struck))].sort();
}

/** One attribute's value as a person reads it: the value, the target, or the first language. */
export function shownValue(attribute: unknown): string {
  if (attribute === null || typeof attribute !== "object") {
    return attribute === undefined ? "" : String(attribute);
  }
  const member = attribute as Record<string, unknown>;
  const raw =
    "value" in member
      ? member.value
      : "object" in member
        ? member.object
        : "languageMap" in member && member.languageMap && typeof member.languageMap === "object"
          ? Object.values(member.languageMap as Record<string, unknown>)[0]
          : member;
  const text = valueWords(raw);
  return text.length > 80 ? `${text.slice(0, 79)}…` : text;
}

/**
 * A structured value in words, never JSON (T-4949): a GeoJSON point as latitude, longitude; any
 * other shape as its type; a postal address as its line; another object as `key: value` pairs.
 */
function valueWords(raw: unknown): string {
  if (raw === null || raw === undefined) return "";
  if (typeof raw !== "object") return String(raw);
  if (Array.isArray(raw)) return raw.map(valueWords).join(", ");
  const record = raw as Record<string, unknown>;
  const at = record.coordinates;
  if (record.type === "Point" && Array.isArray(at) && typeof at[0] === "number" && typeof at[1] === "number") {
    return `${at[1].toFixed(5)}, ${at[0].toFixed(5)}`;
  }
  if (typeof record.type === "string" && "coordinates" in record) return record.type;
  const part = (key: string) => (typeof record[key] === "string" ? (record[key] as string).trim() : "");
  const address = [part("streetAddress"), [part("postalCode"), part("addressLocality")].filter(Boolean).join(" ")]
    .filter(Boolean)
    .join(", ");
  if (address) return address;
  return Object.entries(record)
    .filter(([, each]) => each !== null && each !== undefined && each !== "")
    .map(([key, each]) => `${key}: ${valueWords(each)}`)
    .join(" · ");
}

/** The last segment of an NGSI-LD id, which is what tells two rows apart on screen. */
function shortId(id: string): string {
  return id.split(":").pop() || id;
}

interface Page {
  entities: ProofEntity[];
  total: number;
}

async function pageOf(response: Response): Promise<Page> {
  if (!response.ok) {
    const problem = (await response.json().catch(() => null)) as { detail?: string } | null;
    throw new ApiError(response.status, problem?.detail || response.statusText || `HTTP ${response.status}`);
  }
  const body: unknown = await response.json();
  const entities = Array.isArray(body)
    ? body.filter((item): item is ProofEntity => typeof item === "object" && item !== null && typeof item.id === "string")
    : [];
  const raw = response.headers.get(RESULTS_COUNT);
  const counted = raw === null ? Number.NaN : Number.parseInt(raw.trim(), 10);
  return { entities, total: Number.isNaN(counted) ? entities.length : counted };
}

/** The same page from the space's canonical surface, under the person's own rights (SP-01). */
async function readOriginal(segment: string, type: string, offset: number): Promise<Page> {
  const query = new URLSearchParams({
    type,
    limit: String(PROOF_PAGE),
    offset: String(offset),
    count: "true",
  });
  return pageOf(
    await globalThis.fetch(
      new Request(`${window.location.origin}/cs/${encodeURIComponent(segment)}/ngsi-ld/v1/entities?${query.toString()}`, {
        headers: { Accept: "application/ld+json" },
      }),
    ),
  );
}

/**
 * The page the endpoint serves today, under its saved filter: all a person the space refuses can
 * be shown, because the preview of a draft answers members of the space only (EP-85).
 */
async function readSaved(slug: string, type: string, offset: number): Promise<Page> {
  const query = new URLSearchParams({
    type,
    limit: String(PROOF_PAGE),
    offset: String(offset),
    count: "true",
  });
  return pageOf(
    await globalThis.fetch(
      new Request(endpointUrl(slug, `/ngsi-ld/v1/entities?${query.toString()}`), {
        headers: { Accept: "application/ld+json" },
      }),
    ),
  );
}

/** One page under the draft, from the Portal's preview through the Endpoint's broker view (EP-85). */
async function readServed(
  endpoint: { project: string; name: string },
  draft: FilterDraft,
  type: string,
  page: { ids?: string[]; limit: number; offset: number },
): Promise<Page> {
  const answer = await unwrap(
    await api.POST("/api/v1/projects/{project}/endpoints/{name}/preview", {
      params: { path: endpoint },
      body: {
        projection:
          draft.classes.length > 0
            ? {
                classes: draft.classes,
                ...(Object.keys(draft.filter).length > 0 ? { filter: draft.filter } : {}),
              }
            : null,
        hiddenAttributes: draft.hiddenAttributes,
        type,
        ...(page.ids ? { id: page.ids } : {}),
        limit: page.limit,
        offset: page.offset,
      },
    }),
  );
  const entities = answer.entities.filter(
    (item): item is ProofEntity => typeof item === "object" && item !== null && typeof item.id === "string",
  );
  return { entities, total: answer.count };
}

/** The value after it has rested for `ms`, so typing does not ask the gateway once per key. */
function useSettled<T>(value: T, ms: number): T {
  // Keyed on the content: a new object holding the same draft is not an edit.
  const key = JSON.stringify(value);
  const [settledKey, setSettledKey] = useState(key);
  useEffect(() => {
    const timer = setTimeout(() => setSettledKey(key), ms);
    return () => clearTimeout(timer);
  }, [key, ms]);
  return useMemo(() => JSON.parse(settledKey) as T, [settledKey]);
}

/**
 * The proof beside the filter editor (T-2776, EP-86): left what the endpoint would serve under the
 * draft, right the same entities as the space holds them, one row per entity.
 *
 * Both sides are read with the person's own session, and neither is a privileged path: the right side
 * is the canonical surface, which answers only what the person may read, and the left side is the
 * Portal's preview, which never serves past the endpoint's Policies and answers only a member of the
 * space. A person the space refuses sees the left side alone.
 */
export function FilterProof({
  project,
  name,
  slug,
  segment,
  draft,
  live,
}: {
  project: string;
  /** The Endpoint's manifest name, which the preview route names it by (API/01 §40). */
  name: string;
  slug: string;
  /** The `{space}` segment of `/cs/{space}` (PF-84). */
  segment: string;
  draft: FilterDraft;
  live: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const types = draft.classes.map((klass) => klass.name);
  const [chosen, setChosen] = useState(types[0] ?? "");
  const type = types.includes(chosen) ? chosen : (types[0] ?? "");
  const [offset, setOffset] = useState(0);
  const [side, setSide] = useState<"served" | "original">("served");
  const settled = useSettled(draft, SETTLE_MS);

  const original = useQuery({
    queryKey: ["filter-proof", "original", segment, type, offset],
    enabled: live && type !== "",
    retry: false,
    queryFn: () => readOriginal(segment, type, offset),
  });
  // The person may not read the space: the proof is the served side alone.
  const readable = original.isSuccess;
  const served = useQuery({
    queryKey: ["filter-proof", "served", slug, type, offset, readable ? original.data.entities.map((e) => e.id) : null, readable ? settled : null],
    enabled: live && type !== "" && !original.isPending,
    retry: false,
    placeholderData: (previous) => previous,
    queryFn: async () => {
      if (!readable) {
        return readSaved(slug, type, offset);
      }
      const ids = original.data.entities.map((entity) => entity.id);
      // The total the draft serves, and the page aligned on the original's ids.
      const [total, page] = await Promise.all([
        readServed({ project, name }, settled, type, { limit: 1, offset: 0 }),
        ids.length > 0
          ? readServed({ project, name }, settled, type, { ids, limit: PROOF_PAGE, offset: 0 })
          : Promise.resolve({ entities: [], total: 0 }),
      ]);
      return { entities: page.entities, total: total.total };
    },
  });

  if (!live) {
    return <p className="text-caption text-fg-muted">{t("endpoints.proof.notLive")}</p>;
  }
  if (type === "") {
    return <p className="text-caption text-fg-muted">{t("endpoints.proof.noType")}</p>;
  }

  const rows = served.data ? alignRows(readable ? original.data.entities : undefined, served.data.entities) : [];
  const hidden = hiddenOf(rows);
  const total = readable ? original.data.total : (served.data?.total ?? 0);
  const refused =
    served.error instanceof ApiError && served.error.status === 400
      ? (served.error.problem?.detail ?? served.error.message)
      : null;

  return (
    <section className="space-y-3" aria-labelledby="filter-proof-title">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <h3 id="filter-proof-title" className="text-body font-semibold">
          {t("endpoints.proof.title")}
        </h3>
        {types.length > 1 ? (
          <Field id="filter-proof-type" label={t("endpoints.proof.type")} className="min-w-40">
            <Select
              id="filter-proof-type"
              value={type}
              onChange={(event) => {
                setChosen(event.target.value);
                setOffset(0);
              }}
            >
              {types.map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </Select>
          </Field>
        ) : null}
      </div>

      {refused ? (
        <Alert tone="danger" role="alert">
          {t("endpoints.proof.refused", { reason: refused })}
        </Alert>
      ) : served.isError ? (
        <Alert tone="danger" role="alert">
          {t("endpoints.proof.failed", {
            reason: served.error instanceof ApiError ? served.error.message : t("app.error.generic"),
          })}
        </Alert>
      ) : null}

      <p role="status" aria-live="polite" className="text-body">
        {served.data === undefined
          ? t("endpoints.proof.loading")
          : readable
            ? t("endpoints.proof.summary", { served: served.data.total, total, type, hidden: hidden.length })
            : t("endpoints.proof.summaryServedOnly", { served: served.data.total, type })}
      </p>
      {!readable && !original.isPending ? (
        <p className="text-caption text-fg-muted">{t("endpoints.proof.originalUnreadable")}</p>
      ) : null}

      {readable ? (
        <Tabs
          id="filter-proof-side"
          label={t("endpoints.proof.sides")}
          variant="pill"
          className="lg:hidden"
          value={side}
          onChange={setSide}
          tabs={[
            { value: "served", label: t("endpoints.proof.served") },
            { value: "original", label: t("endpoints.proof.original") },
          ]}
        />
      ) : null}

      <div {...(readable ? tabPanelProps("filter-proof-side", side) : {})}>
        {/* A page of entities with every attribute was 8,500 px of this page (T-5015): the frame scrolls instead. */}
        <Table caption={t("endpoints.proof.caption", { type })} zebra={false} maxHeight="max-h-160" className="table-fixed text-caption">
          <TableHead>
            <TableHeaderCell className={clsx(readable && side !== "served" && "hidden lg:table-cell")}>
              {t("endpoints.proof.served")}
            </TableHeaderCell>
            {readable ? (
              <TableHeaderCell className={clsx(side !== "original" && "hidden lg:table-cell")}>
                {t("endpoints.proof.original")}
              </TableHeaderCell>
            ) : null}
          </TableHead>
          <TableBody>
            {rows.length === 0 && served.data ? (
              <TableRow>
                <TableCell colSpan={readable ? 2 : 1} className="text-fg-muted">
                  {t("endpoints.proof.empty")}
                </TableCell>
              </TableRow>
            ) : null}
            {rows.map((row) => (
              <TableRow key={row.id} data-testid="proof-row">
                <TableCell className={clsx(readable && side !== "served" && "hidden lg:table-cell")}>
                  {row.served ? (
                    <EntityCell entity={row.served} struck={[]} />
                  ) : (
                    <p className="text-fg-muted">
                      <span className="font-mono">{shortId(row.id)}</span> · {t("endpoints.proof.notServed")}
                    </p>
                  )}
                </TableCell>
                {readable ? (
                  <TableCell
                    className={clsx(
                      side !== "original" && "hidden lg:table-cell",
                      row.served === undefined && "bg-surface-muted text-fg-muted",
                    )}
                  >
                    {row.original ? <EntityCell entity={row.original} struck={row.struck} /> : null}
                    {row.served === undefined ? (
                      <p className="mt-1 font-medium">{t("endpoints.proof.notServedMark")}</p>
                    ) : null}
                  </TableCell>
                ) : null}
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          variant="secondary"
          size="sm"
          disabled={offset === 0}
          disabledReason={t("app.firstPage")}
          onClick={() => setOffset(Math.max(0, offset - PROOF_PAGE))}
        >
          {t("endpoints.proof.previous")}
        </Button>
        <Button
          type="button"
          variant="secondary"
          size="sm"
          disabled={offset + PROOF_PAGE >= total}
          disabledReason={t("app.lastPage")}
          onClick={() => setOffset(offset + PROOF_PAGE)}
        >
          {t("endpoints.proof.next")}
        </Button>
        {total > 0 ? (
          <span className="text-caption text-fg-muted">
            {t("endpoints.proof.window", { from: offset + 1, to: Math.min(offset + PROOF_PAGE, total), total })}
          </span>
        ) : null}
      </div>
    </section>
  );
}

/** One entity's attributes; a struck one is what the space holds and the endpoint would not serve. */
function EntityCell({ entity, struck }: { entity: ProofEntity; struck: string[] }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div>
      <p className="font-mono">{shortId(entity.id)}</p>
      <dl className="mt-1 grid grid-cols-[minmax(0,auto)_minmax(0,1fr)] gap-x-2">
        {attributesOf(entity).map((name) => {
          const hidden = struck.includes(name);
          const Wrap = hidden ? "del" : "span";
          return (
            <div key={name} className="contents">
              <dt className={clsx("font-medium", hidden && "text-fg-muted")}>
                <Wrap>{name}</Wrap>
                {hidden ? <span className="sr-only"> ({t("endpoints.proof.hiddenMark")})</span> : null}
              </dt>
              <dd className={clsx("truncate", hidden && "text-fg-muted")}>
                <Wrap>{shownValue(entity[name])}</Wrap>
              </dd>
            </div>
          );
        })}
      </dl>
    </div>
  );
}
