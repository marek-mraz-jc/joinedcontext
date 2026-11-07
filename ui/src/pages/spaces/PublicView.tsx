/**
 * A data view published as a public link (T-3108, API/01 §33, ADR-N-042 §3.5); `SharePanel`
 * publishes it. `PublicViewPage` is the link: a read-only page whose every read is an anonymous
 * read of that Endpoint, so the gateway decides what the link shows.
 */
import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ApiError } from "../../api/client";
import { columnLabel, displayOrder } from "../../components/entities/displayOrder";
import { endpointUrl } from "../../components/endpoints/links";
import {
  Alert,
  Button,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";

/** The rows a published view shows. */
export const PUBLIC_ROWS = 100;


/** A refused or failed anonymous read: its status, and the seconds a 429 asks to wait. */
export class PublicReadError extends ApiError {
  readonly retryAfter?: number;

  constructor(status: number, message: string, retryAfter?: number) {
    super(status, message);
    this.retryAfter = retryAfter;
  }
}

/** `Retry-After` in seconds (RFC 9110 §10.2.3): delay seconds or an HTTP date; absent when neither. */
export function retryAfterSeconds(header: string | null, now = Date.now()): number | undefined {
  if (!header) return undefined;
  const text = header.trim();
  if (/^\d+$/.test(text)) return Number(text);
  const at = Date.parse(text);
  return Number.isNaN(at) ? undefined : Math.max(0, Math.ceil((at - now) / 1000));
}

/** One anonymous read of a public Endpoint; the gateway decides what it answers. */
async function readPublic(slug: string, path: string): Promise<unknown> {
  const response = await globalThis.fetch(
    new Request(endpointUrl(slug, path), { headers: { Accept: "application/json" }, credentials: "omit" }),
  );
  if (!response.ok) {
    throw new PublicReadError(
      response.status,
      response.statusText || `HTTP ${response.status}`,
      retryAfterSeconds(response.headers.get("Retry-After")),
    );
  }
  return response.json();
}

/** Asked again only when the service failed (5xx) or never answered: a refusal stays a refusal. */
export function retryPublic(failures: number, error: unknown): boolean {
  return failures < 2 && (!(error instanceof ApiError) || error.status >= 500);
}

/** The back-off between the two retries: 0.5 s, then 1 s. */
export const publicRetryDelay = (attempt: number): number => 500 * 2 ** attempt;

/** What a failed read tells the visitor, one sentence per kind of failure. */
export function failureText(error: unknown, t: (key: string, options?: Record<string, unknown>) => string, name?: string): string {
  const status = error instanceof ApiError ? error.status : 0;
  if (status === 404) return t("spaces.share.notPublished");
  if (status === 401 || status === 403) return t("spaces.share.closed");
  if (status === 429) {
    const seconds = error instanceof PublicReadError ? error.retryAfter : undefined;
    return seconds === undefined ? t("spaces.share.busyLater") : t("spaces.share.busy", { seconds });
  }
  return name ? t("spaces.share.down", { name }) : t("spaces.share.downUnnamed");
}

/** The text of one language in a JSON-LD value: the page's language, its base, English, the first. */
export function inLanguage(value: unknown, locale: string): string | undefined {
  const items = (Array.isArray(value) ? value : [value]).flatMap((item) =>
    typeof item === "string"
      ? [{ lang: "", text: item }]
      : item && typeof item === "object" && typeof (item as { "@value"?: unknown })["@value"] === "string"
        ? [{ lang: String((item as { "@language"?: unknown })["@language"] ?? ""), text: (item as { "@value": string })["@value"] }]
        : [],
  );
  const base = locale.split("-")[0];
  return (
    items.find((item) => item.lang === locale) ??
    items.find((item) => item.lang === base) ??
    items.find((item) => item.lang === "en") ??
    items[0]
  )?.text;
}

/** `urn:ngsi-ld:WeatherObserved:bcn:X4` as a visitor reads it: `X4`; the whole id stays its title. */
export function localPart(id: string): string {
  return id.split(":").at(-1) || id;
}

/** The value of one key-value attribute as text. */
function textOf(value: unknown): string {
  if (value === null || value === undefined) return "";
  return typeof value === "object" ? JSON.stringify(value) : String(value);
}

/** A failed read with the button that asks again. */
function Failure({ error, name, onRetry }: { error: unknown; name?: string; onRetry: () => void }): JSX.Element {
  const { t } = useTranslation();
  const again = !(error instanceof ApiError) || error.status >= 500 || error.status === 429;
  return (
    <Alert role="alert" tone={error instanceof ApiError && error.status === 404 ? "info" : "danger"}>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <span>{failureText(error, t, name)}</span>
        {again ? (
          <Button size="sm" variant="secondary" onClick={onRetry}>
            {t("app.error.retry")}
          </Button>
        ) : null}
      </div>
    </Alert>
  );
}

/** The page at `/v/{slug}`: what the public Endpoint answers anybody, read-only. */
export function PublicViewPage({ slug }: { slug: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const [chosen, setChosen] = useState("");
  // The Endpoint's own catalogue entry (its DCAT index, EP-08) names the view; without it the type does.
  const entry = useQuery({
    queryKey: ["public-view", slug, "entry"],
    retry: retryPublic,
    retryDelay: publicRetryDelay,
    queryFn: async () => (await readPublic(slug, "/")) as { "dct:title"?: unknown },
  });
  const types = useQuery({
    queryKey: ["public-view", slug, "types"],
    retry: retryPublic,
    retryDelay: publicRetryDelay,
    queryFn: async () => {
      const body = (await readPublic(slug, "/ngsi-ld/v1/types")) as { typeList?: unknown } | unknown[];
      const list = Array.isArray(body) ? body : (body as { typeList?: unknown }).typeList;
      return (Array.isArray(list) ? list : []).filter((item): item is string => typeof item === "string");
    },
  });
  const type = types.data?.includes(chosen) ? chosen : (types.data?.[0] ?? "");
  const rows = useQuery({
    queryKey: ["public-view", slug, "rows", type],
    enabled: type !== "",
    retry: retryPublic,
    retryDelay: publicRetryDelay,
    queryFn: async () => {
      const body = await readPublic(
        slug,
        `/ngsi-ld/v1/entities?type=${encodeURIComponent(type)}&limit=${PUBLIC_ROWS}&options=keyValues`,
      );
      return (Array.isArray(body) ? body : []) as Record<string, unknown>[];
    },
  });
  const columns = useMemo(
    () =>
      displayOrder(
        [],
        [...new Set((rows.data ?? []).flatMap((row) => Object.keys(row)))].filter(
          (key) => !["id", "type", "@context"].includes(key),
        ),
      ),
    [rows.data],
  );
  const locale = i18n.language;
  const typeLabel = (name: string) => columnLabel(undefined, name, locale);
  const name = inLanguage(entry.data?.["dct:title"], locale);
  const title = name ?? (type ? typeLabel(type) : t("spaces.share.heading"));

  // Every state keeps the page's one heading: the catalogue's title, never the link's slug.
  if (types.isPending || types.isError || types.data.length === 0) {
    return (
      <div className="flex flex-col gap-3" data-testid="public-view">
        <h1 className="text-title font-semibold text-fg">{title}</h1>
        <p className="text-body text-fg-muted">{t("spaces.share.publicLead")}</p>
        {types.isPending ? (
          <p className="text-body text-fg-muted" role="status">
            {t("app.loading")}
          </p>
        ) : (
          <Failure error={types.isError ? types.error : new ApiError(404, "Not Found")} name={name} onRetry={() => void types.refetch()} />
        )}
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-3" data-testid="public-view">
      <h1 className="text-title font-semibold text-fg">{title}</h1>
      <p className="text-body text-fg-muted">{t("spaces.share.publicLead")}</p>
      {types.data.length > 1 ? (
        <div className="flex flex-wrap gap-2" role="group" aria-label={t("spaces.share.types")}>
          {types.data.map((name) => (
            <Button key={name} size="sm" variant={name === type ? "primary" : "secondary"} aria-pressed={name === type} onClick={() => setChosen(name)}>
              {typeLabel(name)}
            </Button>
          ))}
        </div>
      ) : null}
      {rows.isPending ? <p role="status">{t("app.loading")}</p> : null}
      {rows.isError ? <Failure error={rows.error} name={title} onRetry={() => void rows.refetch()} /> : null}
      {rows.data ? (
        <>
          <p className="text-caption text-fg-muted">
            {t(rows.data.length >= PUBLIC_ROWS ? "spaces.share.first" : "spaces.share.count", { count: rows.data.length })}
          </p>
          <Table caption={typeLabel(type)}>
            <TableHead>
              <TableHeaderCell className="normal-case! tracking-normal">{t("entityGrid.id")}</TableHeaderCell>
              {columns.map((column) => (
                <TableHeaderCell key={column} className="normal-case! tracking-normal">
                  {columnLabel(undefined, column, locale)}
                </TableHeaderCell>
              ))}
            </TableHead>
            <TableBody>
              {rows.data.map((row) => (
                <TableRow key={String(row.id)}>
                  <TableCell className="whitespace-nowrap font-mono text-caption" title={String(row.id)}>
                    {localPart(String(row.id))}
                  </TableCell>
                  {columns.map((column) => (
                    <TableCell key={column} className="break-words">
                      {textOf(row[column])}
                    </TableCell>
                  ))}
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </>
      ) : null}
    </div>
  );
}
