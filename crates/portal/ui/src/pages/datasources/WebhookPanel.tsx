import { useRef, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import { Alert, Button, Card, CodeBlock, Field, Select, Textarea } from "../../components/ui";

const failureOf = (error: unknown, fallback: string): string | null =>
  error instanceof ApiError ? (error.problem?.detail ?? error.message) : error ? fallback : null;

/** One address with its copy button; when the clipboard refuses, the address is selected for Ctrl+C. */
function Address({ pipeline, url }: { pipeline: string; url: string }): JSX.Element {
  const { t } = useTranslation();
  const block = useRef<HTMLPreElement>(null);
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(true);
    } catch {
      const selection = window.getSelection();
      if (block.current && selection) selection.selectAllChildren(block.current);
    }
  };
  return (
    <li className="flex min-w-0 flex-col gap-1">
      <span className="text-caption font-medium text-fg">{pipeline}</span>
      <div className="flex min-w-0 flex-col items-start gap-2 sm:flex-row">
        <CodeBlock ref={block} label={pipeline} className="w-full min-w-0 flex-1 whitespace-pre-wrap break-all rounded border border-border bg-surface-subtle px-2 py-1 font-mono text-caption">
          {url}
        </CodeBlock>
        <Button type="button" variant="secondary" size="sm" onClick={() => void copy()}>
          {copied ? t("datasources.webhook.copied") : t("datasources.webhook.copy")}
        </Button>
      </div>
    </li>
  );
}

/**
 * A webhook DataSource's addresses and its test event (PL-122). The addresses and the pipeline
 * choices come from the server, which lists only pipelines that read this source; the test event
 * is signed server-side, so the credential never reaches the browser.
 */
export function WebhookPanel({ project, name }: { project: string; name: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const info = useQuery({
    queryKey: ["datasource-webhook", project, name],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/datasources/{name}/webhook", {
          params: { path: { project, name } },
        }),
      ),
  });
  const [pipeline, setPipeline] = useState("");
  const [body, setBody] = useState("");
  const send = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/datasources/{name}/test-event", {
          params: { path: { project, name } },
          body: { pipeline: chosen, body },
        }),
      ),
  });
  const data = info.data;
  const addresses = data?.addresses ?? [];
  // The first reader until the person picks another; a name the list lost falls back too.
  const chosen = addresses.some((address) => address.pipeline === pipeline) ? pipeline : (addresses[0]?.pipeline ?? "");
  const loadFailure = failureOf(info.error, t("app.error.generic"));
  const sendFailure = failureOf(send.error, t("app.error.generic"));
  const answer = send.data;

  return (
    // A part of the DataSource's own section, which takes its name from the heading below.
    <Card padding="md" className="flex flex-col gap-3" data-testid="datasource-webhook">
      <h2 id="datasource-webhook-heading" className="text-body font-medium">
        {t("datasources.webhook.title")}
      </h2>
      <p className="text-caption text-fg-muted">{t("datasources.webhook.hint")}</p>
      {info.isPending ? <p className="text-caption text-fg-muted">{t("app.loading")}</p> : null}
      {loadFailure ? <p className="text-body text-danger-fg">{loadFailure}</p> : null}
      {data ? (
        <>
          <ul className="flex flex-col gap-1 text-caption sm:flex-row sm:gap-x-6">
            <li className="font-mono">{t("datasources.webhook.header", { header: data.header })}</li>
            <li>{t("datasources.webhook.maxBody", { bytes: data.maxBodyBytes.toLocaleString(i18n.language) })}</li>
          </ul>
          {!data.published ? (
            <p className="text-body text-fg-muted">{t("datasources.webhook.unpublished")}</p>
          ) : addresses.length === 0 ? (
            <p className="text-body text-fg-muted">{t("datasources.webhook.noReaders")}</p>
          ) : (
            <ul className="flex flex-col gap-2">
              {addresses.map((address) => (
                <Address key={address.pipeline} pipeline={address.pipeline} url={address.url ?? ""} />
              ))}
            </ul>
          )}
        </>
      ) : null}
      {data && addresses.length > 0 ? (
        <form
          className="flex flex-col gap-3 border-t border-border pt-3"
          aria-labelledby="datasource-webhook-test-heading"
          onSubmit={(event) => {
            event.preventDefault();
            send.mutate();
          }}
        >
          <h3 id="datasource-webhook-test-heading" className="text-body font-medium">
            {t("datasources.webhook.test.title")}
          </h3>
          <p className="text-caption text-fg-muted">{t("datasources.webhook.test.hint")}</p>
          <Field id="webhook-test-pipeline" label={t("datasources.webhook.testPipeline.label")} help={t("datasources.webhook.testPipeline.help")} required>
            <Select id="webhook-test-pipeline" value={chosen} onChange={(event) => setPipeline(event.target.value)}>
              {addresses.map((address) => (
                <option key={address.pipeline} value={address.pipeline}>
                  {address.pipeline}
                </option>
              ))}
            </Select>
          </Field>
          <Field id="webhook-test-body" label={t("datasources.webhook.testBody.label")} help={t("datasources.webhook.testBody.help")} required>
            <Textarea id="webhook-test-body" rows={4} value={body} onChange={(event) => setBody(event.target.value)} spellCheck={false} />
          </Field>
          <div>
            <Button type="submit" variant="primary" loading={send.isPending}>
              {send.isPending ? t("datasources.webhook.test.running") : t("datasources.webhook.test.button")}
            </Button>
          </div>
          {/* The Alert's tone gives it its role; this region only keeps a place for it. */}
          <div aria-live="polite">
            {sendFailure ? <Alert tone="danger">{sendFailure}</Alert> : null}
            {answer && !sendFailure ? (
              answer.status >= 200 && answer.status < 300 ? (
                <Alert tone="success">{t("datasources.webhook.test.accepted", { status: answer.status })}</Alert>
              ) : (
                <Alert tone="danger" title={t("datasources.webhook.test.refused", { status: answer.status, answer: answer.answer })}>
                  {answer.status === 401 ? t("datasources.webhook.test.unauthorized") : null}
                </Alert>
              )
            ) : null}
          </div>
        </form>
      ) : null}
    </Card>
  );
}
