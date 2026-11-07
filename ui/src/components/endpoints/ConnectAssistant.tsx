import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { CopyUrlButton, hubUrl } from "./links";
import { CodeBlock } from "../ui";

/** The name every client lists the hub under, the same as the open-data catalogue's panel. */
const NAME = "joinedcontext";

/**
 * One ready line per MCP client for the hub (T-4114, EP-113): the Claude Code command, the
 * VS Code and Cursor install links and a plain `mcpServers` entry. Each names the hub alone,
 * never an Endpoint, a token or a key, so one installation serves every Endpoint (EP-110).
 */
export function mcpInstall(hub: string): { claude: string; json: string; vscode: string; cursor: string } {
  const server = { type: "http", url: hub };
  return {
    claude: `claude mcp add --transport http ${NAME} ${hub}`,
    json: JSON.stringify({ mcpServers: { [NAME]: server } }, null, 2),
    vscode: `vscode:mcp/install?${encodeURIComponent(JSON.stringify({ name: NAME, ...server }))}`,
    cursor: `cursor://anysphere.cursor-deeplink/mcp/install?name=${NAME}&config=${encodeURIComponent(
      btoa(JSON.stringify({ url: hub })),
    )}`,
  };
}

/** "Connect an AI assistant": the hub's URL and a line per client, each with its copy button. */
export function ConnectAssistant({
  domain,
  heading = true,
}: {
  domain: string | undefined;
  /** A dialog names the content in its own title (T-6095): the section then drops its heading. */
  heading?: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const hub = hubUrl(domain);
  const lines = mcpInstall(hub);
  const link = "text-body text-primary-soft-fg underline hover:no-underline";
  return (
    <section aria-labelledby={heading ? "connect-all" : undefined} className="flex flex-col gap-3">
      <div>
        {heading ? (
          <h2 id="connect-all" className="text-title font-semibold text-fg">
            {t("endpoints.hub.title")}
          </h2>
        ) : null}
        <p className="mt-1 text-body text-fg-muted">{t("endpoints.hub.lead")}</p>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <code className="break-all font-mono text-caption">{hub}</code>
        <CopyUrlButton url={hub} />
      </div>
      <div className="flex flex-wrap items-center gap-4">
        <a className={link} href={lines.vscode}>
          {t("endpoints.hub.vscode")}
        </a>
        <a className={link} href={lines.cursor}>
          {t("endpoints.hub.cursor")}
        </a>
      </div>
      <div className="flex flex-col gap-1">
        <CodeBlock
          label={t("endpoints.hub.claudeLine")}
          className="whitespace-pre-wrap break-all rounded bg-surface-muted p-2 font-mono text-caption"
        >
          {lines.claude}
        </CodeBlock>
        <div>
          <CopyUrlButton url={lines.claude} label={t("endpoints.hub.claude")} />
        </div>
      </div>
      <div className="flex flex-col gap-1">
        <CodeBlock
          label={t("endpoints.hub.jsonLine")}
          className="overflow-x-auto rounded bg-surface-muted p-2 font-mono text-caption"
        >
          {lines.json}
        </CodeBlock>
        <div>
          <CopyUrlButton url={lines.json} label={t("endpoints.hub.json")} />
        </div>
      </div>
    </section>
  );
}
