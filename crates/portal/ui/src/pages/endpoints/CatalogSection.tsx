import { useState } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { CodeBlock, Alert, Badge, Button, Dialog } from "../../components/ui";
import { safeHref } from "../../components/ui";
import { dutyOf, licenceIri, offerSentences } from "./catalog";
import type { CatalogManifest } from "./catalog";

/** The three raw documents a harvester reads, by the `Accept` the gateway negotiates on. */
const RAW = [
  { key: "jsonld", path: "/", accept: "application/ld+json" },
  { key: "turtle", path: "/", accept: "text/turtle" },
  { key: "odrl", path: "/access", accept: "application/odrl+json" },
] as const;

type Raw = (typeof RAW)[number];

/** One text of a language map, as the rest of the page shows it: English, else the first. */
function text(map: Record<string, string> | undefined): string | undefined {
  if (!map) {
    return undefined;
  }
  return map.en ?? Object.values(map).find((value) => value.trim() !== "");
}

function Fact({ label, children }: { label: string; children: ReactNode }): JSX.Element {
  return (
    // min-w-0 and anywhere: a code-list IRI in a fact made the page 409 px wide at 400 (T-5592).
    <div className="min-w-0 rounded-md border border-border p-3">
      <dt className="text-caption font-medium text-fg-muted">{label}</dt>
      <dd className="mt-1 text-sm text-fg [overflow-wrap:anywhere]">{children}</dd>
    </div>
  );
}

function External({ href, children }: { href: string; children: ReactNode }): JSX.Element {
  const safe = safeHref(href);
  if (!safe) {
    return <span className="break-all">{children}</span>;
  }
  return (
    <a href={safe} target="_blank" rel="noreferrer" className="break-all text-primary-soft-fg underline hover:no-underline">
      {children}
    </a>
  );
}

/**
 * What a catalogue says about this endpoint, for a person (T-2789, EP-78, EP-79): who publishes it,
 * under what licence, how often it changes, what it covers and where it comes from, and the offer
 * the licence makes as sentences. The raw record and policy are one click away, read the way a
 * harvester reads them.
 */
export function CatalogSection({
  slug,
  catalog,
  audience,
}: {
  slug: string;
  catalog: CatalogManifest | undefined;
  audience: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const language = (i18n.resolvedLanguage ?? i18n.language ?? "en").slice(0, 2);
  const [raw, setRaw] = useState<{ which: Raw; body?: string; failed?: boolean } | null>(null);

  const open = async (which: Raw) => {
    setRaw({ which });
    try {
      const response = await globalThis.fetch(
        new Request(new URL(`/api/endpoint/${encodeURIComponent(slug)}${which.path}`, window.location.origin), {
          credentials: "same-origin",
          headers: { Accept: which.accept },
        }),
      );
      if (!response.ok) {
        setRaw({ which, failed: true });
        return;
      }
      const body = await response.text();
      const pretty = which.key === "turtle" ? body : JSON.stringify(JSON.parse(body), null, 2);
      setRaw({ which, body: pretty });
    } catch {
      setRaw({ which, failed: true });
    }
  };

  if (!catalog) {
    return <p className="text-body text-fg-muted">{t("endpoints.page.catalog.notCatalogued")}</p>;
  }
  const publisher = text(catalog.publisher?.name);
  const period = catalog.temporal;
  const duty = dutyOf(catalog.license);

  return (
    <div className="space-y-3">
      <dl className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <Fact label={t("endpoints.page.catalog.publisher")}>
          {publisher ? (
            catalog.publisher?.uri ? <External href={catalog.publisher.uri}>{publisher}</External> : publisher
          ) : (
            <span className="text-fg-muted">{t("endpoints.page.notSet")}</span>
          )}
        </Fact>
        <Fact label={t("endpoints.page.catalog.licence")}>
          {catalog.license && duty ? (
            <External href={licenceIri(catalog.license)}>{t(`choice.licence.${catalog.license}`)}</External>
          ) : (
            <span className="text-fg-muted">{t("endpoints.page.notSet")}</span>
          )}
        </Fact>
        <Fact label={t("endpoints.page.catalog.frequency")}>
          {catalog.frequency ? t(`choice.frequency.${catalog.frequency}`) : <span className="text-fg-muted">{t("endpoints.page.notSet")}</span>}
        </Fact>
        <Fact label={t("endpoints.page.catalog.contact")}>
          {catalog.contactPoint?.email ? (
            <a href={`mailto:${catalog.contactPoint.email}`} className="text-primary-soft-fg underline hover:no-underline">
              {catalog.contactPoint.name || catalog.contactPoint.email}
            </a>
          ) : (
            <span className="text-fg-muted">{t("endpoints.page.notSet")}</span>
          )}
        </Fact>
        {catalog.spatial?.length || period ? (
          <Fact label={t("endpoints.page.catalog.coverage")}>
            {catalog.spatial?.length ? <span className="block">{catalog.spatial.join(", ")}</span> : null}
            {period ? (
              <span className="block">
                {t("endpoints.page.catalog.period", {
                  start: period.start ?? "…",
                  end: period.end ?? t("endpoints.page.catalog.ongoing"),
                })}
              </span>
            ) : null}
          </Fact>
        ) : null}
        {catalog.source?.length || catalog.pipelineRef?.name ? (
          <Fact label={t("endpoints.page.catalog.origin")}>
            <ul className="space-y-1">
              {(catalog.source ?? []).map((source) => (
                <li key={source.url}>
                  <External href={source.url ?? ""}>{text(source.title) ?? source.url}</External>
                </li>
              ))}
              {catalog.pipelineRef?.name ? (
                <li>{t("endpoints.page.catalog.byPipeline", { name: catalog.pipelineRef.name })}</li>
              ) : null}
            </ul>
          </Fact>
        ) : null}
        {catalog.themes?.length ? (
          <Fact label={t("endpoints.page.catalog.themes")}>
            <span className="flex flex-wrap gap-1">
              {catalog.themes.map((theme) => (
                <Badge key={theme}>{t(`choice.dataTheme.${theme}`)}</Badge>
              ))}
            </span>
          </Fact>
        ) : null}
      </dl>

      <div className="space-y-1" data-testid="catalog-offer">
        <h3 className="text-sm font-semibold">{t("endpoints.page.catalog.offer")}</h3>
        {offerSentences(t, catalog, audience, language).map((sentence) => (
          <p key={sentence} className="text-body text-fg">
            {sentence}
          </p>
        ))}
      </div>

      <div className="flex flex-wrap gap-2">
        {RAW.map((which) => (
          <Button key={which.key} variant="secondary" size="sm" onClick={() => void open(which)}>
            {t(`endpoints.page.catalog.raw.${which.key}`)}
          </Button>
        ))}
      </div>

      <Dialog
        open={raw !== null}
        onOpenChange={(next) => {
          if (!next) {
            setRaw(null);
          }
        }}
        title={raw ? t(`endpoints.page.catalog.raw.${raw.which.key}`) : ""}
        description={t("endpoints.page.catalog.raw.lead")}
        closeLabel={t("app.close")}
        size="lg"
      >
        {raw?.failed ? (
          <Alert tone="danger">{t("endpoints.page.catalog.raw.failed")}</Alert>
        ) : raw?.body === undefined ? (
          <p role="status" className="text-body text-fg-muted">
            {t("app.loading")}
          </p>
        ) : (
          <CodeBlock label={t("codeBlock.answer")} className="max-h-96 whitespace-pre-wrap break-all rounded-md bg-surface-subtle p-3 font-mono text-caption">
            {raw.body}
          </CodeBlock>
        )}
      </Dialog>
    </div>
  );
}
