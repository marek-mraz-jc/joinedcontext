import type { ResourceProposal } from "../../api/manifest";
import { useState } from "react";
import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { ApiError, forPeople } from "../../api/client";
import { proposeChecked } from "../../api/proposal";
import { Badge } from "../../components/ui/Badge";
import { Button, buttonClass } from "../../components/ui/Button";
import { Card, InlineError } from "../../components/ui";

/**
 * An indicator kept up to date (AG-74, PL-45, PL-51): the pipeline the assistant drafted, what
 * it reads and writes, when it runs and what its test said, and, for an indicator space the
 * project does not have yet, that space's drafts with one action that proposes them with the
 * person's own session. The pipeline itself is proposed from its form, which the assistant has
 * opened. Every string comes from the run's stream and is drawn as text (AG-46).
 */

export interface DraftedManifest {
  kind: string;
  name: string;
  plural: string;
  manifest: Record<string, unknown>;
}

export interface KpiPipeline {
  name: string;
  title?: string;
  formula: string;
  trigger: string;
  sourceEndpoint: string;
  targetSpace: string;
  /** `true` passed, `false` failed or untested; `untested` says why no test ran. */
  verdict: { ok: boolean; untested?: string; findings: string[] };
  drafts: DraftedManifest[];
  runnerAudience?: string;
}

const text = (value: unknown): string | undefined => (typeof value === "string" ? value : undefined);

/** The pipeline of a `draft_kpi_pipeline` step, or none when the payload is not what the Portal wrote. */
export function kpiPipelineOf(output: unknown): KpiPipeline | null {
  if (typeof output !== "object" || output === null) {
    return null;
  }
  const value = output as Record<string, unknown>;
  const name = text(value.name);
  const formula = text(value.formula);
  const trigger = text(value.trigger);
  const sourceEndpoint = text(value.sourceEndpoint);
  const targetSpace = text(value.targetSpace);
  if (!name || !formula || !trigger || !sourceEndpoint || !targetSpace) {
    return null;
  }
  const verdict = (typeof value.verdict === "object" && value.verdict !== null ? value.verdict : {}) as Record<
    string,
    unknown
  >;
  const findings = Array.isArray(verdict.findings)
    ? verdict.findings
        .map((f) => (typeof f === "object" && f !== null ? text((f as Record<string, unknown>).message) : undefined))
        .filter((m): m is string => m !== undefined)
        .map(forPeople)
    : [];
  const drafts = Array.isArray(value.drafts)
    ? value.drafts.filter((d): d is DraftedManifest => {
        const draft = d as Record<string, unknown> | null;
        return (
          typeof draft === "object" &&
          draft !== null &&
          typeof draft.kind === "string" &&
          typeof draft.name === "string" &&
          typeof draft.plural === "string" &&
          typeof draft.manifest === "object" &&
          draft.manifest !== null
        );
      })
    : [];
  return {
    name,
    title: text(value.title),
    formula,
    trigger,
    sourceEndpoint,
    targetSpace,
    verdict: { ok: verdict.ok === true, untested: text(verdict.untested), findings },
    drafts,
    runnerAudience: text(value.runnerAudience),
  };
}

type Proposal =
  | { kind: "idle" }
  | { kind: "proposing" }
  | { kind: "proposed"; count: number }
  | { kind: "refused"; draft: string; detail: string };

export function KpiPipelineCard({ project, pipeline }: { project: string; pipeline: KpiPipeline }): JSX.Element {
  const { t } = useTranslation();
  const [proposal, setProposal] = useState<Proposal>({ kind: "idle" });

  // In order: the space before its endpoint, the endpoint before the policies that name the space.
  const proposeSpace = async (): Promise<void> => {
    setProposal({ kind: "proposing" });
    let count = 0;
    for (const draft of pipeline.drafts) {
      try {
        // A Portal with a forge answers a Change; one without writes the manifest directly.
        await proposeChecked(
          project,
          draft.plural,
          draft.manifest as ResourceProposal,
          true,
        );
        count += 1;
      } catch (err) {
        setProposal({
          kind: "refused",
          draft: `${draft.kind} ${draft.name}`,
          detail: err instanceof ApiError ? (err.problem?.detail ?? err.message) : t("app.error.generic"),
        });
        return;
      }
    }
    setProposal({ kind: "proposed", count });
  };

  return (
    <Card as="section" padding="sm" aria-label={t("agentRun.kpiPipeline.title")} className="flex flex-col gap-2 text-body">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone="accent">{t("agentRun.kpiPipeline.badge")}</Badge>
        <span className="min-w-0 break-words font-medium">{pipeline.title ?? pipeline.name}</span>
      </div>
      <dl className="grid gap-1 text-caption sm:grid-cols-[auto_1fr] sm:gap-x-3">
        <dt className="text-fg-muted">{t("agentRun.kpi.formula")}</dt>
        <dd className="break-words font-mono">{pipeline.formula}</dd>
        <dt className="text-fg-muted">{t("agentRun.kpiPipeline.route")}</dt>
        <dd className="break-words font-mono">
          {pipeline.sourceEndpoint} → {pipeline.targetSpace}
        </dd>
        <dt className="text-fg-muted">{t("agentRun.kpiPipeline.when")}</dt>
        <dd>{pipeline.trigger}</dd>
      </dl>
      <p data-testid="kpi-pipeline-verdict" className={pipeline.verdict.ok ? "text-caption text-success" : "text-caption text-fg-muted"}>
        {pipeline.verdict.ok
          ? t("agentRun.kpiPipeline.tested")
          : pipeline.verdict.untested
            ? t("agentRun.kpiPipeline.untested", { reason: pipeline.verdict.untested })
            : t("agentRun.kpiPipeline.failed")}
      </p>
      {pipeline.verdict.findings.length > 0 ? (
        <ul className="list-inside list-disc text-caption text-danger">
          {pipeline.verdict.findings.map((finding) => (
            <li key={finding}>{finding}</li>
          ))}
        </ul>
      ) : null}
      {pipeline.drafts.length > 0 ? (
        <div className="flex flex-col gap-1 rounded border border-border p-2 text-caption">
          <p>{t("agentRun.kpiPipeline.newSpace", { space: pipeline.targetSpace })}</p>
          <ul className="font-mono">
            {pipeline.drafts.map((draft) => (
              <li key={`${draft.kind}/${draft.name}`}>
                {draft.kind} {draft.name}
              </li>
            ))}
          </ul>
          {pipeline.runnerAudience ? (
            <p className="text-fg-muted">{t("agentRun.kpiPipeline.audience", { slug: pipeline.runnerAudience })}</p>
          ) : null}
          {proposal.kind === "refused" ? (
            <InlineError>
              {t("agentRun.kpiPipeline.refused", { draft: proposal.draft, detail: proposal.detail })}
            </InlineError>
          ) : null}
          {proposal.kind === "proposed" ? (
            <p className="text-success">{t("agentRun.kpiPipeline.proposed", { count: proposal.count })}</p>
          ) : (
            <div>
              <Button
                variant="primary"
                size="sm"
                disabled={proposal.kind === "proposing"}
                disabledReason={t("app.busyReason")}
                onClick={() => {
                  void proposeSpace();
                }}
              >
                {proposal.kind === "proposing" ? t("agentRun.kpiPipeline.proposing") : t("agentRun.kpiPipeline.propose")}
              </Button>
            </div>
          )}
        </div>
      ) : null}
      <div>
        <Link
          to="/projects/$project/$plural"
          params={{ project, plural: "pipelines" }}
          className={buttonClass("secondary", "sm")}
        >
          {t("agentRun.kpiPipeline.open")}
        </Link>
      </div>
    </Card>
  );
}
