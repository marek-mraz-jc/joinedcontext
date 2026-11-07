import { Card } from "../../components/ui";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { forPeople } from "../../api/client";
import { Badge } from "../../components/ui/Badge";
import { isRecord, QueryAnswer, viewOf } from "./QueryResultCard";

/**
 * What a changed pipeline's or data source's test ran on and saw before its form opened (AG-77,
 * PL-45, MF-39): tested with the records it read or mapped, not passed with the findings, or why
 * no test ran. Every string comes from the run's stream and is drawn as text (AG-46).
 */
export interface ChangeTest {
  source: { dataSource?: string; endpoint?: string; url?: string };
  outcome: "passed" | "failed" | "untested";
  /** The findings of a test that did not pass, or why no test ran. */
  reasons: string[];
  records?: number;
  /** A data source's first record, or the entities a pipeline mapped. */
  sample?: unknown;
}

const words = (value: unknown): string | undefined => (typeof value === "string" ? value : undefined);
const count = (value: unknown): number | undefined => (typeof value === "number" ? value : undefined);

/** The test of a `change_resource` step, or none when the step carries none. */
export function changeTestOf(payload: Record<string, unknown>): ChangeTest | null {
  if (payload.tool !== "change_resource" || !isRecord(payload.output) || !isRecord(payload.output.test)) {
    return null;
  }
  const test = payload.output.test;
  const from = isRecord(test.source) ? test.source : {};
  const source = { dataSource: words(from.dataSource), endpoint: words(from.endpoint), url: words(from.url) };
  if (isRecord(test.probe)) {
    const skipped = words(test.probe.skipped);
    return {
      source,
      outcome: skipped === undefined ? "passed" : payload.status === "failed" ? "failed" : "untested",
      reasons: skipped === undefined ? [] : [skipped],
      records: count(test.probe.records),
      sample: test.probe.sample,
    };
  }
  const untested = words(test.untested);
  if (untested !== undefined) {
    return { source, outcome: "untested", reasons: [untested] };
  }
  const verdict = isRecord(test.verdict) ? test.verdict : {};
  const findings = Array.isArray(verdict.findings) ? verdict.findings : [];
  return {
    source,
    outcome: verdict.ok === true ? "passed" : "failed",
    reasons: findings
      .filter(isRecord)
      .map((finding) => [words(finding.path), forPeople(words(finding.message) ?? "")].filter(Boolean).join(" "))
      .filter((reason) => reason !== ""),
    records: count(test.records),
    sample: Array.isArray(test.sample) && test.sample.length > 0 ? test.sample : undefined,
  };
}

export function ChangeTestCard({ test }: { test: ChangeTest }): JSX.Element {
  const { t, i18n } = useTranslation();
  const { dataSource, endpoint, url } = test.source;
  const ranOn =
    dataSource !== undefined
      ? t("agentRun.changeTest.dataSource", { name: dataSource })
      : endpoint !== undefined
        ? t("agentRun.changeTest.endpoint", { name: endpoint })
        : url !== undefined
          ? t("agentRun.changeTest.url", { url })
          : null;
  const tone = test.outcome === "passed" ? "success" : test.outcome === "failed" ? "danger" : "neutral";
  return (
    <Card as="section" padding="sm" aria-label={t("agentRun.changeTest.title")} className="flex flex-col gap-2 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={tone}>{t(`agentRun.changeTest.${test.outcome}`)}</Badge>
        {ranOn !== null ? <span className="min-w-0 break-words">{ranOn}</span> : null}
      </div>
      {test.records !== undefined ? (
        <p className="text-xs text-fg-muted">{t("agentRun.changeTest.records", { count: test.records })}</p>
      ) : null}
      {test.reasons.length > 0 ? (
        <ul className={test.outcome === "failed" ? "list-inside list-disc text-xs text-danger" : "list-inside list-disc text-xs text-fg-muted"}>
          {test.reasons.map((reason) => (
            <li key={reason} className="break-words">
              {reason}
            </li>
          ))}
        </ul>
      ) : null}
      {test.sample !== undefined ? <QueryAnswer view={viewOf(test.sample, i18n.resolvedLanguage ?? i18n.language)} /> : null}
    </Card>
  );
}
