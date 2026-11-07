import { CodeBlock, Card } from "../../components/ui";
import { useState } from "react";
import type { JSX } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { rememberPrefill } from "../../assistant/state";
import { refName } from "../../api/manifest";
import { servedRepresentations } from "../../components/endpoints/links";
import { AUDIENCE_TONE } from "../../components/endpoints/sharing";
import { TypeLink } from "../models/ModelLinks";
import { Badge } from "../../components/ui/Badge";
import type { BadgeTone } from "../../components/ui/Badge";
import { Button } from "../../components/ui/Button";

/**
 * What the assistant drafted when asked to share data (EP-72, UI-45): the endpoint as the form
 * will take it, the slug the platform minted (read-only: a person never types one), the
 * audience, the representations, the attributes held back, the draft policies, and the one
 * action that matters, opening the form to review and propose. Every string is untrusted text
 * from the run's stream (AG-46).
 */

export interface Proposal {
  lane: string;
  slug: string;
  endpoint: { metadata?: { name?: string; title?: Record<string, string> }; spec?: Record<string, unknown> };
  policies: unknown[];
  prefill: Record<string, unknown>;
}

const LANE_TONE: Record<string, BadgeTone> = { green: "success", yellow: "warning", red: "danger" };

/** The proposal of a `propose_endpoint` step, or none when the payload is not what the Portal wrote. */
export function proposalOf(output: unknown): Proposal | null {
  if (typeof output !== "object" || output === null) {
    return null;
  }
  const value = output as Record<string, unknown>;
  if (
    typeof value.slug !== "string" ||
    typeof value.lane !== "string" ||
    typeof value.endpoint !== "object" ||
    value.endpoint === null ||
    typeof value.prefill !== "object" ||
    value.prefill === null
  ) {
    return null;
  }
  return {
    lane: value.lane,
    slug: value.slug,
    endpoint: value.endpoint as Proposal["endpoint"],
    policies: Array.isArray(value.policies) ? value.policies : [],
    prefill: value.prefill as Record<string, unknown>,
  };
}

function strings(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((v): v is string => typeof v === "string") : [];
}

export function EndpointProposalCard({
  project,
  proposal,
}: {
  project: string;
  proposal: Proposal;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const [copied, setCopied] = useState<number | null>(null);
  const spec = proposal.endpoint.spec ?? {};
  const name = proposal.endpoint.metadata?.name ?? String(proposal.prefill.name ?? "");
  const title = proposal.endpoint.metadata?.title;
  const audience = typeof spec.audience === "string" ? spec.audience : "project-list";
  const projects = strings(spec.allowedProjects);
  const representations = servedRepresentations(spec);
  const hidden = strings((spec.projection as { hiddenAttributes?: unknown } | undefined)?.hiddenAttributes);
  const types = proposal.policies.flatMap((policy) => {
    const info = (policy as { spec?: { information?: { entities?: { type?: unknown }[] }[] } }).spec?.information;
    return Array.isArray(info)
      ? info.flatMap((i) => (i.entities ?? []).map((e) => e.type)).filter((x): x is string => typeof x === "string")
      : [];
  });
  const route = `/projects/${project}/endpoints`;

  const review = (): void => {
    rememberPrefill(route, proposal.prefill);
    void navigate({ to: "/projects/$project/$plural", params: { project, plural: "endpoints" } });
  };

  const copy = async (index: number, policy: unknown): Promise<void> => {
    try {
      await navigator.clipboard.writeText(JSON.stringify(policy, null, 2));
      setCopied(index);
    } catch {
      setCopied(null);
    }
  };

  return (
    <Card as="section" padding="sm" aria-label={t("agentRun.proposal.title")} className="flex flex-col gap-2 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone="primary">Endpoint</Badge>
        <span className="min-w-0 break-words font-medium">
          {title?.[i18n.language] ?? title?.en ?? name}
        </span>
        <Badge tone={LANE_TONE[proposal.lane] ?? "neutral"}>{t("agentRun.proposal.lane", { lane: proposal.lane })}</Badge>
      </div>
      <p className="text-xs text-fg-muted">{t("agentRun.proposal.lead")}</p>
      <dl className="grid gap-1 text-xs sm:grid-cols-[auto_1fr] sm:gap-x-3">
        <dt className="text-fg-muted">{t("agentRun.proposal.slug")}</dt>
        <dd className="break-all font-mono">{proposal.slug}</dd>
        <dt className="text-fg-muted">{t("agentRun.proposal.audience")}</dt>
        <dd className="flex flex-wrap gap-1">
          <Badge tone={AUDIENCE_TONE[audience] ?? "neutral"}>{audience}</Badge>
          {projects.length > 0 ? <span>{t("agentRun.proposal.projects", { projects: projects.join(", ") })}</span> : null}
        </dd>
        <dt className="text-fg-muted">{t("agentRun.proposal.representations")}</dt>
        <dd className="flex flex-wrap gap-1">
          {representations.map((r) => (
            <Badge key={r} mono>
              {r}
            </Badge>
          ))}
        </dd>
        {hidden.length > 0 ? (
          <>
            <dt className="text-fg-muted">{t("agentRun.proposal.hidden")}</dt>
            <dd className="flex flex-wrap gap-1">
              {hidden.map((attribute) => (
                <span key={attribute} className="font-mono line-through decoration-danger">
                  {attribute}
                </span>
              ))}
            </dd>
          </>
        ) : null}
        {types.length > 0 ? (
          <>
            <dt className="text-fg-muted">{t("agentRun.proposal.types")}</dt>
            <dd className="flex flex-wrap gap-x-2 font-mono">
              {[...new Set(types)].map((type) => (
                <TypeLink key={type} project={project} type={type} space={refName(spec.contextSpaceRef) || undefined} />
              ))}
            </dd>
          </>
        ) : null}
      </dl>
      {proposal.policies.length > 0 ? (
        <details className="text-xs">
          <summary className="cursor-pointer">{t("agentRun.proposal.policies", { n: proposal.policies.length })}</summary>
          <div className="mt-1 space-y-2">
            {proposal.policies.map((policy, index) => (
              <div key={index}>
                <CodeBlock label={t("codeBlock.json")} className="max-h-48 whitespace-pre-wrap break-words rounded bg-surface-subtle p-2 font-mono">
                  {JSON.stringify(policy, null, 2)}
                </CodeBlock>
                <Button
                  size="sm"
                  onClick={() => {
                    void copy(index, policy);
                  }}
                >
                  {copied === index ? t("agentRun.proposal.copied") : t("agentRun.proposal.copyPolicy")}
                </Button>
              </div>
            ))}
          </div>
        </details>
      ) : null}
      <div>
        <Button variant="primary" size="sm" onClick={review}>
          {t("agentRun.proposal.review")}
        </Button>
      </div>
    </Card>
  );
}
