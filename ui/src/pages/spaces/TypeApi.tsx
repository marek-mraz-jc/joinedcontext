/**
 * The API of one entity type of a space (T-3110, ADR-N-042 §3.6): the calls each Endpoint of the
 * space answers for that type, one tried live with the person's session, how a program gets its
 * own scoped token (a ServiceAccount), and the webhooks that watch the type, which are NGSI-LD
 * subscriptions proposed as Changes like every other manifest.
 */
import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { putDraft } from "../../api/drafts";
import { asManifests } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { useBranding } from "../../branding";
import {
  CopyUrlButton,
  endpointUrl,
  REPRESENTATION_PATHS,
  representationUrl,
  servedRepresentations,
} from "../../components/endpoints/links";
import { CodeBlock, Alert, Button, Card, ExternalLink, Field, Select } from "../../components/ui";

/** The calls one Endpoint answers for one type, as a program writes them. */
export function callsOf(
  slug: string,
  type: string,
  served: string[],
  domain: string | undefined,
): { key: string; url: string }[] {
  const t = encodeURIComponent(type);
  const calls = [
    { key: "list", url: endpointUrl(slug, `/ngsi-ld/v1/entities?type=${t}&limit=10`) },
    { key: "count", url: endpointUrl(slug, `/ngsi-ld/v1/entities?type=${t}&count=true&limit=0`) },
    { key: "one", url: endpointUrl(slug, `/ngsi-ld/v1/entities/{id}`) },
    { key: "history", url: endpointUrl(slug, `/ngsi-ld/v1/temporal/entities?type=${t}&lastN=10`) },
  ];
  // Only the ones with a path of their own: an OData or WebSocket client is not a link.
  for (const representation of served.filter((rep) => rep !== "ngsi-ld" && REPRESENTATION_PATHS[rep])) {
    calls.push({ key: representation, url: representationUrl(slug, representation, domain) });
  }
  return calls;
}

/**
 * The query of the view on screen as each representation of one Endpoint takes it (T-4884): the
 * NGSI-LD query itself, and OGC API Features and OData with the same `q` as their `ngsild.q`
 * option (ADR-0028 rule 6), so all three answer the same rows. Only the representations the
 * Endpoint serves.
 */
export function viewQueryUrls(
  slug: string,
  type: string,
  q: string | undefined,
  served: string[],
): { key: "ngsi-ld" | "ogc-features" | "odata"; url: string }[] {
  const name = encodeURIComponent(type);
  const asked = q?.trim() ? encodeURIComponent(q.trim()) : "";
  const urls: { key: "ngsi-ld" | "ogc-features" | "odata"; url: string }[] = [
    { key: "ngsi-ld", url: endpointUrl(slug, `/ngsi-ld/v1/entities?type=${name}${asked ? `&q=${asked}` : ""}`) },
  ];
  if (served.includes("ogc-features")) {
    urls.push({
      key: "ogc-features",
      url: endpointUrl(slug, `/ogc/features/collections/${name}/items${asked ? `?ngsild.q=${asked}` : ""}`),
    });
  }
  if (served.includes("odata")) {
    urls.push({ key: "odata", url: endpointUrl(slug, `/odata/v4/${name}${asked ? `?ngsild.q=${asked}` : ""}`) });
  }
  return urls;
}

/** The subscriptions of the space that watch the type: by its label and an entity selector. */
export function webhooksOf(subscriptions: Manifest[], space: string, type: string): Manifest[] {
  return subscriptions.filter((subscription) => {
    const labels = subscription.metadata.labels ?? {};
    const spec = subscription.spec as { contextSpaceRef?: unknown; entities?: { type?: unknown }[] };
    const inSpace = labels["joinedcontext.com/space"] === space || spec.contextSpaceRef === space;
    return inSpace && (spec.entities ?? []).some((selector) => selector.type === type);
  });
}

/** One address with what it is and a button that copies it. */
function UrlRow({ label, url, copy }: { label: string; url: string; copy: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <li className="flex flex-wrap items-center gap-x-3 gap-y-1 py-1">
      <span className="w-28 shrink-0 text-caption font-semibold text-fg-muted">{label}</span>
      <code className="order-last min-w-0 basis-full font-mono text-caption text-fg [overflow-wrap:anywhere] sm:order-none sm:flex-1 sm:basis-0">{url}</code>
      <CopyUrlButton url={url} label={t("spaces.api.copy")} spoken={copy} />
    </li>
  );
}

/** A draft name a new webhook can take: the type in lower case and four random letters. */
function draftName(type: string): string {
  const base = type.toLowerCase().replace(/[^a-z0-9-]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 40) || "type";
  return `webhook-${base}-${Math.random().toString(36).slice(2, 6)}`;
}

export function TypeApi({
  project,
  space,
  type,
  endpoints,
  q,
}: {
  project: string;
  space: string;
  type: string;
  /** The space's Endpoints. */
  endpoints: Manifest[];
  /** The NGSI-LD query of the view the person set up, none for the whole type. */
  q?: string;
}): JSX.Element {
  const { t } = useTranslation();
  const { domain } = useBranding();
  const navigate = useNavigate();
  const [tried, setTried] = useState<{ slug: string; status: number; text: string } | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const subscriptions = useQuery({
    queryKey: queryKeys.list(project, "subscriptions"),
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "subscriptions" } },
        }),
      ),
  });
  const hooks = webhooksOf(asManifests(subscriptions.data?.items ?? []), space, type);
  const served = endpoints.filter((endpoint) => typeof (endpoint.spec as { slug?: unknown }).slug === "string");
  // One Endpoint at a time: a space with 60 of them printed 21 000 px of the same calls (T-5064).
  const [chosen, setChosen] = useState<string | null>(null);
  const shown = served.find((endpoint) => endpoint.metadata.name === chosen) ?? served[0];

  const tryIt = async (slug: string) => {
    setFailed(null);
    try {
      const response = await globalThis.fetch(
        new Request(endpointUrl(slug, `/ngsi-ld/v1/entities?type=${encodeURIComponent(type)}&limit=1`), {
          headers: { Accept: "application/ld+json" },
        }),
      );
      const text = await response.text();
      let shown = text;
      try {
        shown = JSON.stringify(JSON.parse(text), null, 2);
      } catch {
        // Not JSON: the answer as it came.
      }
      setTried({ slug, status: response.status, text: shown.slice(0, 8000) });
    } catch (error) {
      setFailed(error instanceof Error ? error.message : String(error));
    }
  };

  const newWebhook = async () => {
    setFailed(null);
    const name = draftName(type);
    try {
      await putDraft(project, "Subscription", name, {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Subscription",
        metadata: { name, namespace: project, labels: { "joinedcontext.com/space": space } },
        spec: { entities: [{ type }], notification: { endpoint: { uri: "", accept: "application/json" } } },
      });
      await navigate({ href: `/projects/${encodeURIComponent(project)}/subscriptions?draft=${encodeURIComponent(name)}` });
    } catch (error) {
      setFailed(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <div className="flex flex-col gap-4" data-testid="view-api">
      <p className="text-body text-fg-muted">{t("spaces.api.lead", { type })}</p>
      {failed ? (
        <Alert role="alert" tone="danger">
          {failed}
        </Alert>
      ) : null}
      {served.length === 0 ? <p className="text-body text-fg-muted">{t("spaces.api.noEndpoint")}</p> : null}
      {served.length > 1 ? (
        <Field id="type-api-endpoint" label={t("spaces.api.pick")} help={t("spaces.api.pickHint", { count: served.length })}>
          <Select id="type-api-endpoint" value={shown?.metadata.name} onChange={(event) => setChosen(event.target.value)}>
            {served.map((endpoint) => (
              <option key={endpoint.metadata.name} value={endpoint.metadata.name}>
                {endpoint.metadata.name}
              </option>
            ))}
          </Select>
        </Field>
      ) : null}
      {(shown ? [shown] : []).map((endpoint) => {
        const slug = (endpoint.spec as { slug: string }).slug;
        return (
          <Card padding="sm" key={endpoint.metadata.name} className="flex flex-col gap-2">
            <h2 className="text-body font-semibold text-fg">
              {t("spaces.api.endpoint", { name: endpoint.metadata.name })}
            </h2>
            <section aria-label={t("spaces.api.viewQuery")} className="flex flex-col gap-1" data-testid="api-view-query">
              <h3 className="text-caption font-semibold text-fg">{t("spaces.api.viewQuery")}</h3>
              <p className="text-caption text-fg-muted">
                {q?.trim() ? t("spaces.api.viewQueryLead", { q: q.trim() }) : t("spaces.api.viewQueryAll", { type })}
              </p>
              <ul className="flex max-w-4xl flex-col gap-1">
                {viewQueryUrls(slug, type, q, servedRepresentations(endpoint.spec as Record<string, unknown>)).map((each) => {
                  const surface = t(`spaces.api.surface.${each.key}`);
                  return <UrlRow key={each.key} label={surface} url={each.url} copy={t("spaces.api.copySurface", { surface })} />;
                })}
              </ul>
            </section>
            <section aria-label={t("spaces.api.calls")} className="flex flex-col gap-1" data-testid="api-calls">
              <h3 className="text-caption font-semibold text-fg">{t("spaces.api.calls")}</h3>
              <ul className="flex max-w-4xl flex-col gap-1">
                {callsOf(slug, type, servedRepresentations(endpoint.spec as Record<string, unknown>), domain).map((call) => {
                  const name = t(`spaces.api.call.${call.key}`, { defaultValue: call.key });
                  return <UrlRow key={call.key} label={name} url={call.url} copy={t("spaces.api.copySurface", { surface: name })} />;
                })}
              </ul>
            </section>
            <CodeBlock label={t("codeBlock.curl")} className="whitespace-pre-wrap rounded-md bg-surface-subtle p-2 font-mono text-caption [overflow-wrap:anywhere]">
              {`curl -H "Authorization: Bearer $TOKEN" \\\n  "${endpointUrl(slug, `/ngsi-ld/v1/entities?type=${encodeURIComponent(type)}&limit=10`)}"`}
            </CodeBlock>
            <div className="flex flex-wrap items-center gap-2">
              <Button size="sm" variant="secondary" onClick={() => void tryIt(slug)}>
                {t("spaces.api.try")}
              </Button>
              <ExternalLink href={endpointUrl(slug, "/")} className="text-caption">
                {t("spaces.api.index")}
              </ExternalLink>
            </div>
            {tried?.slug === slug ? (
              <div className="flex flex-col gap-1">
                <p className="text-caption text-fg-muted" role="status">
                  {t("spaces.api.answered", { status: tried.status })}
                </p>
                <CodeBlock label={t("codeBlock.answer")} className="max-h-64 rounded-md bg-surface-subtle p-2 font-mono text-caption" data-testid="api-tried">
                  {tried.text}
                </CodeBlock>
              </div>
            ) : null}
          </Card>
        );
      })}

      <section aria-labelledby="api-tokens" className="flex flex-col gap-1">
        <h2 id="api-tokens" className="text-body font-semibold text-fg">
          {t("spaces.api.tokens")}
        </h2>
        <p className="text-body text-fg-muted">{t("spaces.api.tokensLead")}</p>
        <Link
          to="/projects/$project/settings/$tab"
          params={{ project, tab: "service-accounts" }}
          className="text-body text-primary-soft-fg underline-offset-2 hover:underline"
        >
          {t("spaces.api.tokensLink")}
        </Link>
      </section>

      <section aria-labelledby="api-webhooks" className="flex flex-col gap-2">
        <h2 id="api-webhooks" className="text-body font-semibold text-fg">
          {t("spaces.api.webhooks")}
        </h2>
        <p className="text-body text-fg-muted">{t("spaces.api.webhooksLead", { type })}</p>
        {hooks.length === 0 ? (
          <p className="text-caption text-fg-muted">{t("spaces.api.noWebhook")}</p>
        ) : (
          <ul className="flex flex-col gap-1">
            {hooks.map((hook) => {
              const uri = (hook.spec as { notification?: { endpoint?: { uri?: unknown } } }).notification?.endpoint?.uri;
              return (
                <li key={hook.metadata.name} className="text-body [overflow-wrap:anywhere]">
                  <span className="font-semibold">{hook.metadata.name}</span>{" "}
                  <span className="font-mono text-caption text-fg-muted">{typeof uri === "string" ? uri : ""}</span>
                </li>
              );
            })}
          </ul>
        )}
        <Button size="sm" variant="secondary" className="w-fit" onClick={() => void newWebhook()}>
          {t("spaces.api.newWebhook")}
        </Button>
      </section>
    </div>
  );
}
