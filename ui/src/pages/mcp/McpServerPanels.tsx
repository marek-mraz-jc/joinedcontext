import { useState } from "react";
import type { JSX } from "react";
import { useQueries } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Badge, Button } from "../../components/ui";
import { mergedTools, memberKey, probe } from "./mcp";
import type { MemberChoice, Probe } from "./mcp";

/** Each member's own MCP surface as this person reaches it now, asked once a minute at most. */
export function useProbes(members: MemberChoice[]) {
  return useQueries({
    queries: members.map((member) => ({
      queryKey: ["mcp-probe", member.slug],
      queryFn: ({ signal }: { signal: AbortSignal }) => probe(member.slug, signal),
      staleTime: 60_000,
      retry: false,
    })),
  });
}

function health(t: (key: string, options?: Record<string, unknown>) => string, result: Probe | undefined): { tone: "success" | "warning" | "danger" | "neutral"; text: string } {
  if (!result) return { tone: "neutral", text: t("mcp.health.checking") };
  if (result.state === "answers") return { tone: "success", text: t("mcp.health.answers", { count: result.tools.length }) };
  if (result.state === "refused") return { tone: "warning", text: t("mcp.health.refused", { status: result.status }) };
  return { tone: "danger", text: t("mcp.health.failed", { reason: result.reason }) };
}

/**
 * The members with their health, and the server's tool list as this person would get it (ADR-N-043
 * §2.3): a tool appears when at least one member that answers offers it. A member this person may
 * not reach adds nothing, exactly as on the server.
 */
export function ToolPreview({ members }: { members: MemberChoice[] }): JSX.Element {
  const { t } = useTranslation();
  const probes = useProbes(members);
  const results = members.map((member, index) => ({ member: memberKey(member), probe: probes[index]?.data }));
  const tools = mergedTools(
    results.filter((entry): entry is { member: string; probe: Probe } => entry.probe !== undefined),
  );
  if (members.length === 0) {
    return <p className="text-caption text-fg-muted">{t("mcp.preview.none")}</p>;
  }
  return (
    <section aria-labelledby="mcp-preview" className="flex flex-col gap-3">
      <h3 id="mcp-preview" className="text-body font-semibold">
        {t("mcp.preview.title")}
      </h3>
      <ul aria-label={t("mcp.preview.members")} className="flex flex-col gap-1">
        {results.map(({ member, probe: result }) => {
          const shown = health(t, result);
          return (
            <li key={member} className="flex flex-wrap items-center justify-between gap-2">
              <span className="font-mono text-caption">{member}</span>
              <Badge tone={shown.tone}>{shown.text}</Badge>
            </li>
          );
        })}
      </ul>
      {tools.length > 0 ? (
        <ul aria-label={t("mcp.preview.tools")} className="flex flex-col gap-2">
          {tools.map(({ tool, members: offering }) => (
            <li key={tool.name} className="rounded-md border border-border p-2">
              <p className="flex flex-wrap items-center gap-2">
                <span className="font-mono font-semibold">{tool.name}</span>
                <Badge tone={tool.readOnly ? "neutral" : "warning"}>{tool.readOnly ? t("mcp.preview.reads") : t("mcp.preview.writes")}</Badge>
              </p>
              {tool.description ? <p className="text-caption text-fg-muted">{tool.description}</p> : null}
              <p className="text-caption">{t("mcp.preview.offeredBy", { members: offering.join(", ") })}</p>
            </li>
          ))}
        </ul>
      ) : probes.every((query) => !query.isPending) ? (
        <p>{t("mcp.preview.noTools")}</p>
      ) : null}
    </section>
  );
}

/** A value with its copy button, the button saying when it copied. */
export function CopyValue({ label, value, code }: { label: string; value: string; code?: boolean }): JSX.Element {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  return (
    <div className="flex flex-col gap-1">
      <span className="text-caption font-semibold">{label}</span>
      <div className="flex flex-wrap items-start gap-2">
        {code ? (
          <pre aria-label={label} className="min-w-0 flex-1 whitespace-pre-wrap break-all rounded-md bg-surface-subtle p-2 text-caption">
            {value}
          </pre>
        ) : (
          <code className="min-w-0 flex-1 break-all rounded-md bg-surface-subtle p-2 text-caption">{value}</code>
        )}
        <Button
          size="sm"
          variant="secondary"
          aria-label={t("mcp.copyOf", { what: label })}
          onClick={() => {
            void navigator.clipboard
              ?.writeText(value)
              .then(() => setCopied(true))
              .catch(() => setCopied(false));
          }}
        >
          {copied ? t("mcp.copied") : t("mcp.copy")}
        </Button>
      </div>
    </div>
  );
}
