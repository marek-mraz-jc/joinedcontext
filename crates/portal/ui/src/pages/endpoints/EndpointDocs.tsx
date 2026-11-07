import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { endpointUrl } from "../../components/endpoints/links";
import { CodeBlock, Table, TableBody, TableCell, TableHead, TableHeaderCell, TableRow } from "../../components/ui";
import { DeveloperPanel, TryIt } from "./DeveloperPanel";
import { EndpointStandards } from "./EndpointStandards";
import { snippetsOf } from "./snippets";

/** What the docs page reads of the gateway's OpenAPI document (EP-99); the rest is not shown. */
interface Parameter {
  name?: string;
  in?: string;
  required?: boolean;
  description?: string;
  schema?: { enum?: unknown[]; type?: unknown };
}
interface OpenApi {
  info?: { title?: string; description?: string; version?: string };
  paths?: Record<string, Record<string, { summary?: string; parameters?: Parameter[] }>>;
  components?: { schemas?: Record<string, { properties?: Record<string, { type?: unknown; description?: unknown }> }> };
}

export interface DocOperation {
  method: string;
  path: string;
  summary: string;
  parameters: { name: string; where: string; required: boolean; description: string; values: string[] }[];
  /** The path of one example call, the first type and an id of it filled in. */
  example: string;
}

/** The operations of the document, each with an example call a person can run as it stands. */
export function operationsOf(doc: OpenApi): DocOperation[] {
  const types = (doc.paths?.["/ngsi-ld/v1/entities"]?.get?.parameters ?? []).find((p) => p.name === "type")?.schema?.enum;
  const first = Array.isArray(types) && typeof types[0] === "string" ? types[0] : undefined;
  return Object.entries(doc.paths ?? {}).flatMap(([path, methods]) =>
    Object.entries(methods).map(([method, operation]) => ({
      method: method.toUpperCase(),
      path,
      summary: operation.summary ?? "",
      parameters: (operation.parameters ?? []).map((p) => ({
        name: p.name ?? "",
        where: p.in ?? "",
        required: p.required === true,
        description: p.description ?? "",
        values: Array.isArray(p.schema?.enum) ? p.schema.enum.filter((v): v is string => typeof v === "string") : [],
      })),
      example:
        path === "/ngsi-ld/v1/entities"
          ? first
            ? `${path}?type=${encodeURIComponent(first)}&limit=5`
            : `${path}?limit=5`
          : path.replace("{entityId}", encodeURIComponent(`urn:ngsi-ld:${first ?? "Thing"}:001`)),
    })),
  );
}

/** Each type the document describes, with its attributes and their types. */
export function typesOf(doc: OpenApi): { name: string; attributes: { name: string; type: string; description: string }[] }[] {
  return Object.entries(doc.components?.schemas ?? {}).map(([name, schema]) => ({
    name,
    attributes: Object.entries(schema.properties ?? {}).map(([attribute, definition]) => ({
      name: attribute,
      type: typeof definition.type === "string" ? definition.type : Array.isArray(definition.type) ? definition.type.join(" | ") : "",
      description: typeof definition.description === "string" ? definition.description : "",
    })),
  }));
}

/**
 * An Endpoint's API documentation (EP-99, T-3265): the gateway's generated OpenAPI document, read
 * as the person (or an anonymous visitor on a public Endpoint) may read it, each operation with
 * its parameters and an example call as curl, Python and JavaScript. On a public Endpoint it is
 * the developer page (T-5111): licence, rate limit, a sample per surface and try-it. The gateway
 * answers the title and description in the page's language (T-5630).
 */
export function EndpointDocs({ slug, open, heading = true }: { slug: string; open: boolean; heading?: boolean }): JSX.Element {
  const { t, i18n } = useTranslation();
  const language = i18n.resolvedLanguage ?? i18n.language;
  const doc = useQuery({
    queryKey: ["endpoint-openapi", slug, language],
    retry: false,
    gcTime: 0,
    queryFn: async () => {
      const answer = await globalThis.fetch(
        new Request(endpointUrl(slug, "/openapi.json"), { credentials: "same-origin", headers: { Accept: "application/json", "Accept-Language": language } }),
      );
      if (!answer.ok) throw new Error(String(answer.status));
      return (await answer.json()) as OpenApi;
    },
  });
  if (doc.isPending || doc.isError) {
    return (
      <div className="flex flex-col gap-2" data-testid="endpoint-docs">
        {heading ? <h1 className="text-title font-semibold text-fg">{t("endpoints.docs.title")}</h1> : null}
        <p className="text-body text-fg-muted" role="status">
          {doc.isPending ? t("app.loading") : t("endpoints.docs.unavailable")}
        </p>
      </div>
    );
  }
  const info = doc.data.info ?? {};
  return (
    <div className="flex flex-col gap-4" data-testid="endpoint-docs">
      {heading ? <h1 className="text-title font-semibold text-fg">{info.title ?? slug}</h1> : null}
      <p className="text-body text-fg-muted">{info.description ?? t("endpoints.docs.lead")}</p>
      <a className="w-fit text-caption text-primary-soft-fg underline" href={endpointUrl(slug, "/openapi.json")}>
        {t("endpoints.docs.download")}
      </a>
      {open ? <DeveloperPanel slug={slug} /> : null}
      {open ? <EndpointStandards slug={slug} /> : null}
      {operationsOf(doc.data).map((operation) => {
        const code = snippetsOf(endpointUrl(slug, operation.example), open);
        return (
          <section key={`${operation.method} ${operation.path}`} aria-label={`${operation.method} ${operation.path}`} className="flex flex-col gap-2 rounded-lg border border-border p-3">
            <h2 className="font-mono text-body font-semibold text-fg">
              {operation.method} {operation.path}
            </h2>
            {operation.summary ? <p className="text-body text-fg-muted">{operation.summary}</p> : null}
            {operation.parameters.length > 0 ? (
              <Table caption={t("endpoints.docs.parametersOf", { path: operation.path })}>
                <TableHead>
                  <TableHeaderCell>{t("endpoints.docs.parameter")}</TableHeaderCell>
                  <TableHeaderCell>{t("endpoints.docs.meaning")}</TableHeaderCell>
                </TableHead>
                <TableBody>
                  {operation.parameters.map((p) => (
                    <TableRow key={p.name}>
                      <TableCell>
                        <span className="font-mono">
                          {p.name}
                          {p.required ? " *" : ""}
                        </span>
                      </TableCell>
                      <TableCell>
                        {p.description}
                        {p.values.length > 0 ? ` (${p.values.join(", ")})` : ""}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            ) : null}
            <CodeBlock label={t("codeBlock.curl")} className="overflow-x-auto rounded-md bg-surface-subtle p-2 font-mono text-caption">{code.curl}</CodeBlock>
            {open && operation.method === "GET" ? <TryIt slug={slug} path={operation.example} accept="application/ld+json" editable /> : null}
          </section>
        );
      })}
      {typesOf(doc.data).map((type) => (
        <section key={type.name} aria-label={type.name} className="flex flex-col gap-1">
          <h2 className="font-mono text-body font-semibold text-fg">{type.name}</h2>
          <ul className="flex flex-col gap-0.5 text-caption">
            {type.attributes.map((attribute) => (
              <li key={attribute.name}>
                <span className="font-mono">{attribute.name}</span>
                {attribute.type ? <span className="text-fg-muted"> · {attribute.type}</span> : null}
                {attribute.description ? <span className="text-fg-muted"> · {attribute.description}</span> : null}
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}
