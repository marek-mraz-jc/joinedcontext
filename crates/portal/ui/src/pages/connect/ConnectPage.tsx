import { useEffect, useId, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { CodeBlock, Alert, Button, Icon, PageHeader, PageLoading } from "../../components/ui";
import { ANONYMOUS_CALLS_PER_MINUTE, clientName, readServer, serverUrl, setups } from "./connect";

/**
 * Puts `text` on the clipboard: the Clipboard API, else (an http origin, a frame without
 * clipboard-write, a refusal) the browser's copy command on a hidden field. False when neither
 * copied, so the caller selects the text for Ctrl+C and says so (T-4762).
 */
async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const area = document.createElement("textarea");
    area.value = text;
    area.readOnly = true;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    try {
      return document.execCommand("copy");
    } catch {
      return false;
    } finally {
      area.remove();
    }
  }
}

/** A block a person copies whole, with a button that says so to a screen reader too. */
function CopyBlock({ label, code }: { label: string; code: string }): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const block = useRef<HTMLPreElement>(null);
  const [copied, setCopied] = useState<"copied" | "selected" | null>(null);
  useEffect(() => {
    if (copied !== "copied") return undefined;
    const timer = window.setTimeout(() => setCopied(null), 2000);
    return () => window.clearTimeout(timer);
  }, [copied]);
  const copy = async () => {
    if (await copyText(code)) {
      setCopied("copied");
      return;
    }
    // Nothing reached the clipboard: the block stays selected for the visitor's own Ctrl+C.
    const selection = window.getSelection();
    if (block.current && selection) {
      selection.selectAllChildren(block.current);
    }
    setCopied("selected");
  };
  return (
    <div className="flex flex-col gap-1">
      <CodeBlock label={t("codeBlock.code")} ref={block} id={id} className="overflow-x-auto whitespace-pre rounded bg-surface-muted p-3 font-mono text-caption">
        {code}
      </CodeBlock>
      <div className="flex items-center gap-2">
        <Button
          size="sm"
          aria-describedby={id}
          icon={<Icon name={copied === "copied" ? "check" : "copy"} className="size-4" />}
          onClick={() => void copy()}
        >
          {label}
        </Button>
        <span role="status" aria-live="polite" className="text-caption text-fg-muted">
          {copied ? t(`connect.${copied}`) : ""}
        </span>
      </div>
    </div>
  );
}

function Client({ id, title, steps, code, copy, prompt }: {
  id: string;
  title: string;
  steps: string;
  code?: string;
  copy: string;
  prompt: string;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <section aria-labelledby={id} className="flex flex-col gap-2 rounded border border-border bg-surface p-4">
      <h3 id={id} className="text-body font-semibold text-fg">
        {title}
      </h3>
      <p className="text-body text-fg">{steps}</p>
      {code ? <CopyBlock label={copy} code={code} /> : null}
      <p className="text-caption text-fg-muted">{t("connect.tryPrompt", { prompt })}</p>
    </section>
  );
}

/**
 * The public page of one named MCP server (T-4169): what it holds, its tools as a stranger sees
 * them, and a setup per AI client. No sign-in; the page never names a token or the hub.
 */
export function ConnectPage({ project, name }: { project: string; name: string }): JSX.Element {
  const { t } = useTranslation();
  // readServer answers every failure as a state of its own, so the query never errors or retries.
  const query = useQuery({
    queryKey: ["connect", project, name],
    queryFn: ({ signal }) => readServer(project, name, signal),
    retry: false,
    refetchOnWindowFocus: false,
  });
  const answer = query.isFetching ? null : (query.data ?? null);

  const url = serverUrl(window.location.hostname, project, name);
  const server = clientName(project, name);
  const lines = setups(url, server);
  const retry = (
    <Button size="sm" variant="secondary" onClick={() => void query.refetch()}>
      {t("connect.retry")}
    </Button>
  );

  if (answer === null) {
    return (
      <section className="mx-auto max-w-3xl space-y-6 px-4 py-6">
        <PageHeader title={t("connect.title")} description={t("connect.lead")} />
        <PageLoading label={t("connect.loading")} />
      </section>
    );
  }
  if (answer.state !== "answers") {
    const message =
      answer.state === "missing"
        ? t("connect.missing")
        : answer.state === "busy"
          ? t("connect.busy", { seconds: answer.retryAfter ?? 60 })
          : t("connect.failed");
    return (
      <section className="mx-auto max-w-3xl space-y-6 px-4 py-6">
        <PageHeader title={t("connect.title")} description={t("connect.lead")} />
        <Alert tone={answer.state === "missing" ? "info" : "warning"} actions={answer.state === "missing" ? undefined : retry}>
          {message}
        </Alert>
      </section>
    );
  }

  const { title, instructions, tools } = answer.server;
  // The server's own words come first; the gateway's note on how fan-out works follows them.
  const about = instructions.split("\n\n")[0] ?? "";
  const prompt = t("connect.prompt", { title });
  return (
    <section className="mx-auto max-w-3xl space-y-8 px-4 py-6" aria-label={t("connect.title")}>
      <div className="space-y-2">
        <PageHeader title={t("connect.title")} description={title} />
        {about ? <p className="text-body text-fg-muted">{about}</p> : null}
      </div>

      <section aria-labelledby="connect-address" className="space-y-2">
        <h2 id="connect-address" className="text-title font-semibold text-fg">
          {t("connect.address")}
        </h2>
        <CopyBlock label={t("connect.copyAddress")} code={url} />
        <ul className="list-disc space-y-1 pl-5 text-body text-fg">
          <li>{t("connect.readOnly")}</li>
          <li>{t("connect.noLogin")}</li>
          <li>{t("connect.limit", { count: ANONYMOUS_CALLS_PER_MINUTE })}</li>
        </ul>
      </section>

      <section aria-labelledby="connect-clients" className="space-y-3">
        <h2 id="connect-clients" className="text-title font-semibold text-fg">
          {t("connect.clients")}
        </h2>
        <Client
          id="connect-claude-code"
          title="Claude Code"
          steps={t("connect.claudeCode.steps")}
          code={lines.claudeCode}
          copy={t("connect.claudeCode.copy")}
          prompt={prompt}
        />
        <Client
          id="connect-claude-desktop"
          title="Claude Desktop"
          steps={t("connect.claudeDesktop.steps")}
          code={lines.claudeDesktop}
          copy={t("connect.claudeDesktop.copy")}
          prompt={prompt}
        />
        <Client
          id="connect-chatgpt"
          title="ChatGPT"
          steps={t("connect.chatgpt.steps")}
          code={url}
          copy={t("connect.copyAddress")}
          prompt={prompt}
        />
        <Client
          id="connect-generic"
          title={t("connect.generic.title")}
          steps={t("connect.generic.steps")}
          code={lines.generic}
          copy={t("connect.generic.copy")}
          prompt={prompt}
        />
      </section>

      <section aria-labelledby="connect-tools" className="space-y-2">
        <h2 id="connect-tools" className="text-title font-semibold text-fg">
          {t("connect.tools", { count: tools.length })}
        </h2>
        {tools.length === 0 ? (
          <p className="text-body text-fg-muted">{t("connect.noTools")}</p>
        ) : (
          <dl className="space-y-3">
            {tools.map((tool) => (
              <div key={tool.name}>
                <dt className="font-mono text-body text-fg">{tool.name}</dt>
                <dd className="text-caption text-fg-muted">{tool.description.split("\n")[0]}</dd>
              </div>
            ))}
          </dl>
        )}
      </section>
    </section>
  );
}
