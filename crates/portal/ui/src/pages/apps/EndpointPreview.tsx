import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { EndpointLink, ENDPOINT_LINKS, endpointUrl } from "../../components/endpoints/links";
import { useAccess } from "../../components/entities/AccessPanel";
import type { GrantDocument } from "../../api/access";
import { writesOf } from "../access/EffectivePermissions";
import { fetchJson } from "../endpoints/SchemaProjectionPanel";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableRowHeaderCell,
} from "../../components/ui";

/** How many entities are worth showing to judge an endpoint by: enough to see the shape (AP-51). */
const SAMPLE_LIMIT = 5;

/**
 * The endpoint's `/access` document as a few sentences: which types, which attributes, and
 * whether the caller's own grant reaches further than a generated app ever will.
 *
 * A grant with no `attributes` and one with `"*"` mean the same thing, every attribute the
 * endpoint publishes, and neither is a list to print (EP-55).
 */
export function accessWords(document: GrantDocument | undefined): {
  types: string[];
  attrs: string[] | "*";
  writes: string[];
  denied: string[];
} {
  const permissions = document?.permissions ?? [];
  const unlimited = permissions.some(
    (entry) => entry.attributes === "*" || entry.attributes === undefined,
  );
  const named = (entries: { attributes?: string[] | "*" }[]) => [
    ...new Set(entries.flatMap((entry) => (Array.isArray(entry.attributes) ? entry.attributes : []))),
  ].sort();
  return {
    types: [
      ...new Set(
        permissions
          .map((entry) => entry.resource?.type)
          .filter((type): type is string => typeof type === "string"),
      ),
    ].sort(),
    attrs: unlimited ? "*" : named(permissions),
    writes: [...new Set(permissions.flatMap((entry) => writesOf(entry.actions ?? [])))].sort(),
    denied: named(document?.prohibitions ?? []),
  };
}

/** One served entity as one line: its local id (the full URN kept for the tooltip), the type, and the first few values. */
function sampleLine(entity: Record<string, unknown>): {
  id: string;
  localId: string;
  type: string;
  values: string;
} {
  const values = Object.entries(entity)
    .filter(([key]) => key !== "id" && key !== "type" && key !== "@context")
    .slice(0, 3)
    .map(([key, value]) => `${key}=${text(value)}`)
    .join(", ");
  const id = String(entity.id ?? "");
  return { id, localId: id.split(":").pop() || id, type: String(entity.type ?? ""), values };
}

/** A value from the gateway, short enough for a table cell. Objects arrive when `keyValues` is ignored. */
function text(value: unknown): string {
  const rendered = typeof value === "object" && value !== null ? JSON.stringify(value) : String(value);
  return rendered.length > 40 ? `${rendered.slice(0, 39)}…` : rendered;
}

/**
 * What the chosen Endpoint actually gives you, before you describe the app (AP-51, UI-35, UI-36).
 *
 * The point is that nobody has to read a manifest to decide: the grant is shown in words, the
 * data in five real entities, and the model behind both is one link away. Every request here
 * goes through the endpoint with the person's own session, so what they see is what they may
 * see, and an endpoint that answers nothing says so instead of blocking the form.
 */
export function EndpointPreview({ slug }: { slug: string }): JSX.Element {
  const { t } = useTranslation();
  const access = useAccess(slug);
  // The samples wait for the grant, because a read names a type (GW33): a query with no
  // selector is 400 BadRequestData, and the first type the grant allows is the one to show.
  const granted = accessWords(access.data).types;
  const samples = useQuery({
    queryKey: ["generator-endpoint-samples", slug, granted[0] ?? ""],
    enabled: slug !== "" && granted.length > 0,
    retry: false,
    // Nothing of one person's data outlives their page, as on the explore page (R17).
    gcTime: 0,
    queryFn: async () =>
      (await fetchJson(
        endpointUrl(
          slug,
          `/ngsi-ld/v1/entities?type=${encodeURIComponent(granted[0])}&limit=${SAMPLE_LIMIT}&options=keyValues`,
        ),
      )) as Record<string, unknown>[],
  });

  const words = accessWords(access.data);
  const rows = (Array.isArray(samples.data) ? samples.data : []).slice(0, SAMPLE_LIMIT);
  const summary = [
    words.types.length > 0
      ? t("apps.generate.preview.readsTypes", { types: words.types.join(", ") })
      : t("apps.generate.preview.reads"),
    words.attrs === "*" ? t("apps.generate.preview.allAttributes") : words.attrs.join(", "),
    words.writes.length > 0
      ? t("apps.generate.preview.alsoWrite", { actions: words.writes.join(", ") })
      : t("apps.generate.preview.readOnly"),
  ].join(" · ");

  return (
    <section
      aria-label={t("apps.generate.preview.title")}
      className="space-y-1.5 rounded border border-border px-3 py-2 text-sm"
    >
      {access.isPending && (
        <p role="status" className="text-xs text-fg-muted">
          {t("apps.generate.preview.loading")}
        </p>
      )}
      {access.isError && <p className="text-xs text-fg-muted">{t("apps.generate.preview.accessUnavailable")}</p>}
      {access.isSuccess && (
        <p>
          {summary}
          {words.denied.length > 0 && (
            <span className="text-fg-muted">
              {" · "}
              {t("apps.generate.preview.denied", { attrs: words.denied.join(", ") })}
            </span>
          )}
        </p>
      )}

      <details>
        <summary className="cursor-pointer text-xs text-fg-muted">
          {rows.length > 0
            ? t("apps.generate.preview.samplesCount", { count: rows.length })
            : t("apps.generate.preview.samples")}
        </summary>
        <div className="mt-1.5">
          {/* No granted type means nothing to ask for: a read names a type (GW33), so the
              endpoint is answered as empty rather than left asking for ever. */}
          {granted.length > 0 && samples.isPending && (
            <p role="status" className="text-xs text-fg-muted">
              {t("apps.generate.preview.loading")}
            </p>
          )}
          {samples.isError && <p className="text-xs text-fg-muted">{t("apps.generate.preview.samplesUnavailable")}</p>}
          {(granted.length === 0 || samples.isSuccess) && rows.length === 0 && (
            <p className="text-xs text-fg-muted">{t("apps.generate.preview.samplesEmpty")}</p>
          )}
          {rows.length > 0 && (
            <Table caption={t("apps.generate.preview.samples")}>
              <TableHead>
                <TableHeaderCell>{t("apps.generate.preview.columnId")}</TableHeaderCell>
                <TableHeaderCell>{t("apps.generate.preview.columnType")}</TableHeaderCell>
                <TableHeaderCell>{t("apps.generate.preview.columnValues")}</TableHeaderCell>
              </TableHead>
              <TableBody>
                {rows.map((entity, index) => {
                  const line = sampleLine(entity);
                  return (
                    <TableRow key={line.id || index}>
                      <TableRowHeaderCell title={line.id} className="break-all font-mono font-normal">
                        {line.localId}
                      </TableRowHeaderCell>
                      <TableCell>{line.type}</TableCell>
                      <TableCell className="break-all font-mono">{line.values}</TableCell>
                    </TableRow>
                  );
                })}
              </TableBody>
            </Table>
          )}
        </div>
      </details>

      <ul className="flex flex-wrap gap-x-2 gap-y-1 text-xs" aria-label={t("apps.generate.preview.documents")}>
        {ENDPOINT_LINKS.map((link) => (
          <li key={link.key}>
            <EndpointLink href={endpointUrl(slug, link.path)} muted>
              {t(`endpoints.link.${link.key}`)}
            </EndpointLink>
          </li>
        ))}
      </ul>
    </section>
  );
}
