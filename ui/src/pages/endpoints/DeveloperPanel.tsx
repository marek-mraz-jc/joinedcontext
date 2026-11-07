import { useId, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import { CopyUrlButton, endpointUrl } from "../../components/endpoints/links";
import { Alert, Button, CodeBlock, Field, Input, Tabs, safeHref, tabPanelProps } from "../../components/ui";
import { samplesOf } from "./developerSamples";
import type { Language, SurfaceSamples } from "./developerSamples";

/** The most characters of an answer the page shows. */
const SHOWN = 4000;
const LANGUAGE_NAMES: Record<Language, string> = { curl: "curl", python: "Python", javascript: "JavaScript" };

/** The standards answer the page and EndpointStandards share: one read per page. */
export function useStandards(slug: string) {
  return useQuery({
    queryKey: ["endpoint-standards", slug],
    retry: false,
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/catalogue/endpoints/{slug}/standards", { params: { path: { slug } } })),
  });
}

/**
 * Why a typed request path may not be sent, or `undefined` when it may: it stays below this
 * Endpoint, so try-it never reaches another path of the host.
 */
export function pathProblem(path: string): "start" | "outside" | undefined {
  if (!path.startsWith("/")) return "start";
  const route = path.split("?", 1)[0] ?? "";
  if (path.length > 2000 || route.includes("//") || /(^|\/)\.\.?(\/|$)/.test(route) || /%2e|%2f|\\|#/i.test(route)) {
    return "outside";
  }
  return undefined;
}

/**
 * One call sent from the page, anonymously (T-5111): the browser sends no cookie, so the answer
 * is what any visitor without a token gets, whoever is signed in to the Portal.
 */
export function TryIt({ slug, path, accept, editable = false }: { slug: string; path: string; accept: string; editable?: boolean }): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const [typed, setTyped] = useState(path);
  const [answer, setAnswer] = useState<{ status: number; text: string; cut: boolean } | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const request = editable ? typed.trim() : path;
  const problem = pathProblem(request);
  const send = async () => {
    if (problem) return;
    setFailed(null);
    setSending(true);
    try {
      const response = await globalThis.fetch(new Request(endpointUrl(slug, request), { credentials: "omit", headers: { Accept: accept } }));
      const text = await response.text();
      let shown = text;
      try {
        shown = JSON.stringify(JSON.parse(text), null, 2);
      } catch {
        // Not JSON: the answer as it came.
      }
      setAnswer({ status: response.status, text: shown.slice(0, SHOWN), cut: shown.length > SHOWN });
    } catch (error) {
      setFailed(error instanceof Error ? error.message : String(error));
    } finally {
      setSending(false);
    }
  };
  return (
    <div className="flex flex-col gap-2">
      {editable ? (
        <Field
          id={`${id}-path`}
          label={t("endpoints.developer.request")}
          help={t("endpoints.developer.requestHint", { example: path })}
          errors={problem ? [t(`endpoints.developer.problem.${problem}`)] : undefined}
        >
          <Input id={`${id}-path`} value={typed} onChange={(event) => setTyped(event.target.value)} spellCheck={false} className="font-mono" />
        </Field>
      ) : null}
      <div className="flex flex-wrap items-center gap-2">
        <Button
          size="sm"
          variant="secondary"
          loading={sending}
          disabled={problem !== undefined}
          disabledReason={problem ? t(`endpoints.developer.problem.${problem}`) : undefined}
          onClick={() => void send()}
        >
          {t("endpoints.developer.send")}
        </Button>
        <span className="text-caption text-fg-muted">{t("endpoints.developer.anonymous")}</span>
      </div>
      {failed ? (
        <Alert role="alert" tone="danger">
          {t("endpoints.developer.failed", { reason: failed })}
        </Alert>
      ) : null}
      {answer ? (
        <div className="flex flex-col gap-1">
          <p className="text-caption text-fg-muted" role="status">
            {t(answer.status === 429 ? "endpoints.developer.limited" : "endpoints.developer.answered", { status: answer.status })}
            {answer.cut ? ` ${t("endpoints.developer.cut", { chars: SHOWN })}` : ""}
          </p>
          <CodeBlock label={t("codeBlock.answer")} className="max-h-80 rounded-md bg-surface-subtle p-2 font-mono text-caption" data-testid="try-it-answer">
            {answer.text}
          </CodeBlock>
        </div>
      ) : null}
    </div>
  );
}

/** The path below the Endpoint of a sample's address; `undefined` for one on another base. */
function below(base: string, url: string): string | undefined {
  const root = base.endsWith("/") ? base : `${base}/`;
  return url.startsWith(root) ? `/${url.slice(root.length)}` : undefined;
}

const ACCEPT: Partial<Record<SurfaceSamples["surface"], string>> = {
  "ngsi-ld": "application/ld+json",
  "ogc-features": "application/json",
  geojson: "application/geo+json",
  odata: "application/json",
  sta: "application/json",
  csv: "text/csv",
  ical: "text/calendar",
};

function Surface({ slug, base, sample }: { slug: string; base: string; sample: SurfaceSamples }): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const languages = (Object.keys(LANGUAGE_NAMES) as Language[]).filter((language) => sample.code[language] !== undefined);
  const [chosen, setChosen] = useState<Language>(languages[0] ?? "curl");
  const language = languages.includes(chosen) ? chosen : (languages[0] ?? "curl");
  const code = sample.code[language] ?? "";
  const path = below(base, sample.url);
  const accept = ACCEPT[sample.surface];
  return (
    <div className="flex flex-col gap-3">
      <p className="text-body text-fg-muted">{t(`endpoints.developer.surface.${sample.surface}`)}</p>
      <p className="break-all font-mono text-caption text-fg">{sample.url}</p>
      <Tabs id={`${id}-language`} label={t("endpoints.developer.language")} tabs={languages.map((value) => ({ value, label: LANGUAGE_NAMES[value] }))} value={language} onChange={setChosen} />
      <div {...tabPanelProps(`${id}-language`, language)} className="flex flex-col gap-2">
        <CodeBlock label={t("endpoints.developer.sample", { language: LANGUAGE_NAMES[language] })} className="overflow-x-auto rounded-md bg-surface-subtle p-2 font-mono text-caption" data-testid={`sample-${sample.surface}-${language}`}>
          {code}
        </CodeBlock>
        <div>
          <CopyUrlButton url={code} label={t("endpoints.developer.copy")} />
        </div>
      </div>
      {sample.tools.map((tool) => (
        <div key={tool.tool} className="flex flex-col gap-1 rounded-md border border-border p-2">
          <h3 className="text-caption font-semibold text-fg">{t(`endpoints.developer.tool.${tool.tool}.title`)}</h3>
          <p className="text-caption text-fg-muted">{t(`endpoints.developer.tool.${tool.tool}.steps`)}</p>
          <p className="break-all font-mono text-caption text-fg">{tool.url}</p>
        </div>
      ))}
      {path && accept ? <TryIt slug={slug} path={path} accept={accept} /> : null}
    </div>
  );
}

/**
 * A public Endpoint's developer page, above its OpenAPI operations (T-5111): the licence and the
 * rate limit a caller works under, then a ready sample per surface the Endpoint serves, each
 * one to copy or to send from the page anonymously.
 */
export function DeveloperPanel({ slug }: { slug: string }): JSX.Element | null {
  const { t } = useTranslation();
  const id = useId();
  const standards = useStandards(slug);
  const data = standards.data;
  const samples = data && Array.isArray(data.served) ? samplesOf(data) : [];
  const [chosen, setChosen] = useState<string>("");
  const surface = samples.find((sample) => sample.surface === chosen) ?? samples[0];
  if (!data || !surface) return null;
  return (
    <div className="flex flex-col gap-4" data-testid="developer-panel">
      <section aria-labelledby={`${id}-terms`} className="flex flex-col gap-2 rounded-lg border border-border p-3">
        <h2 id={`${id}-terms`} className="text-body font-semibold text-fg">
          {t("endpoints.developer.terms")}
        </h2>
        <dl className="grid grid-cols-1 gap-x-4 gap-y-1 text-body sm:grid-cols-[max-content_1fr]">
          <dt className="font-semibold text-fg">{t("endpoints.developer.licence")}</dt>
          <dd className="text-fg-muted" data-testid="developer-licence">
            {data.licence ? (
              <a className="text-primary-soft-fg underline" href={safeHref(data.licence.url)} rel="noreferrer">
                {data.licence.title}
              </a>
            ) : (
              t("endpoints.developer.noLicence")
            )}
          </dd>
          <dt className="font-semibold text-fg">{t("endpoints.developer.limit")}</dt>
          <dd className="text-fg-muted" data-testid="developer-limit">
            {typeof data.requestsPerMinute === "number"
              ? t("endpoints.developer.perMinute", { rate: data.requestsPerMinute })
              : t("endpoints.developer.noLimit")}
          </dd>
          <dt className="font-semibold text-fg">{t("endpoints.developer.access")}</dt>
          <dd className="text-fg-muted">{t("endpoints.developer.accessText")}</dd>
        </dl>
      </section>
      <section aria-labelledby={`${id}-samples`} className="flex flex-col gap-3 rounded-lg border border-border p-3">
        <h2 id={`${id}-samples`} className="text-body font-semibold text-fg">
          {t("endpoints.developer.samples")}
        </h2>
        <Tabs
          id={`${id}-surface`}
          label={t("endpoints.developer.samples")}
          tabs={samples.map((sample) => ({ value: sample.surface, label: t(`endpoints.developer.surfaceName.${sample.surface}`) }))}
          value={surface.surface}
          onChange={setChosen}
          className="overflow-x-auto"
        />
        <div {...tabPanelProps(`${id}-surface`, surface.surface)}>
          <Surface key={surface.surface} slug={slug} base={data.base} sample={surface} />
        </div>
      </section>
    </div>
  );
}
