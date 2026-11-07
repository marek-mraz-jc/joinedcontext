import { reasonOf } from "../../api/client";
import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Alert, Button, Card } from "../../components/ui";
import { download } from "../models/diagramExport";
import {
  conversationKeys,
  conversationUsage,
  costText,
  exportConversations,
  forgetAnswers,
  msText,
  shareText,
  tokensText,
  usdText,
} from "./conversations";
import type { ConversationFilter, Spread, UsageSummary as Usage } from "./conversations";

const PHASES = ["route", "work", "answer"] as const;

/** Whether a speed was measured: a figure of "–" over "median –, p90 –" says nothing (T-4994). */
function measured(spread: Spread | undefined): boolean {
  return spread?.p50 !== null && spread?.p50 !== undefined;
}

/** Whether any question of the range carries a price; without one, the cost figures are left out. */
function priced(s: Usage): boolean {
  return s.costUsd.total !== null && (s.costUsd.total > 0 || s.uncostedQuestions === 0);
}

/** Whether the provider reported any tokens for the range. */
function counted(s: Usage): boolean {
  return (s.tokens.in ?? 0) + (s.tokens.out ?? 0) > 0;
}

/** Whether the second row has a figure to show. */
function timed(s: Usage): boolean {
  return (
    measured(s.firstTokenMs) || (s.answerCache?.hits ?? 0) > 0 || PHASES.some((phase) => measured(s.phases[phase]))
  );
}

/** What the range lacks, said once above the figures instead of "unknown" in each of them. */
function notRecorded(s: Usage): "both" | "cost" | "tokens" | null {
  if (!priced(s) && !counted(s)) return "both";
  if (!priced(s)) return "cost";
  if (!counted(s)) return "tokens";
  return null;
}

/** One figure of the summary: a label over its value, and a line under it. */
function Figure({ label, value, note }: { label: string; value: string; note?: string }): JSX.Element {
  return (
    <div className="min-w-0">
      <dt className="text-caption text-fg-muted">{label}</dt>
      <dd className="text-lg font-semibold tabular-nums">{value}</dd>
      {note ? <dd className="text-caption text-fg-muted">{note}</dd> : null}
    </div>
  );
}

/**
 * The usage of the filtered turns (API/05 §3.1, T-4728): questions, cost per question with the
 * unpriced counted apart, tokens, speed per phase, timeouts, cost per stage and the newest days,
 * computed by the assistant in one aggregate; and the turns as CSV. Cards, so a phone reads it.
 */
export function UsageSummary({ project, filter }: { project: string; filter: ConversationFilter }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.language;
  const [exporting, setExporting] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  const [forgetting, setForgetting] = useState(false);
  const [forgot, setForgot] = useState<number | null>(null);
  const usage = useQuery({
    queryKey: conversationKeys.usage(project, filter),
    queryFn: () => conversationUsage(project, filter),
  });
  const unknown = t("conversations.unknownPrice");
  const spread = (s: Spread | undefined) =>
    t("conversations.usage.spread", { p50: msText(s?.p50), p90: msText(s?.p90), max: msText(s?.max) });
  const percent = (n: number, of: number) => (of > 0 ? shareText(n / of, locale) : "–");

  const save = async () => {
    setExporting(true);
    setFailed(null);
    try {
      const csv = await exportConversations(project, filter);
      download(csv, `${project}-conversations.csv`, "text/csv;charset=utf-8");
    } catch (error) {
      setFailed(reasonOf(error, t("app.error.generic")));
    } finally {
      setExporting(false);
    }
  };

  const deployment = filter.deployment;
  const forget = async (name: string) => {
    setForgetting(true);
    setFailed(null);
    setForgot(null);
    try {
      setForgot(await forgetAnswers(project, name));
    } catch (error) {
      setFailed(reasonOf(error, t("app.error.generic")));
    } finally {
      setForgetting(false);
    }
  };

  const s = usage.data;
  return (
    <Card padding="md" className="space-y-4" aria-label={t("conversations.usage.title")}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="text-base font-semibold">{t("conversations.usage.title")}</h2>
        <div className="flex flex-wrap gap-2">
          {deployment ? (
            <Button
              variant="secondary"
              size="sm"
              onClick={() => void forget(deployment)}
              disabled={forgetting}
              title={t("conversations.usage.forgetHint")}
            >
              {forgetting ? t("conversations.usage.forgetting") : t("conversations.usage.forget")}
            </Button>
          ) : null}
          <Button
            variant="secondary"
            size="sm"
            onClick={() => void save()}
            disabled={exporting}
            title={exporting ? t("conversations.usage.exporting") : undefined}
          >
            {exporting ? t("conversations.usage.exporting") : t("conversations.usage.export")}
          </Button>
        </div>
      </div>
      {forgot !== null ? <p role="status">{t("conversations.usage.forgot", { count: forgot })}</p> : null}
      {failed ? <Alert tone="danger">{failed}</Alert> : null}
      {usage.isError ? <Alert tone="danger">{reasonOf(usage.error, t("app.error.generic"))}</Alert> : null}
      {usage.isLoading ? <p role="status">{t("app.loading")}</p> : null}
      {s && s.questions === 0 ? <p className="text-fg-muted">{t("conversations.usage.none")}</p> : null}
      {s && s.questions > 0 ? (
        <>
          {notRecorded(s) ? <p className="text-caption text-fg-muted">{t(`conversations.usage.notRecorded.${notRecorded(s)}`)}</p> : null}
          <dl className="grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-6">
            <Figure label={t("conversations.usage.questions")} value={String(s.questions)} />
            {priced(s) ? (
              <Figure
                label={t("conversations.usage.cost")}
                value={costText(s.costUsd.total, s.uncostedQuestions, unknown)}
                note={
                  s.uncostedQuestions > 0
                    ? t("conversations.usage.uncosted", { count: s.uncostedQuestions })
                    : undefined
                }
              />
            ) : null}
            {priced(s) ? (
              <Figure
                label={t("conversations.usage.perQuestion")}
                value={usdText(s.costUsd.mean)}
                note={t("conversations.usage.costSpread", { p50: usdText(s.costUsd.p50), p90: usdText(s.costUsd.p90) })}
              />
            ) : null}
            {counted(s) ? (
              <Figure
                label={t("conversations.usage.tokens")}
                value={`${tokensText(s.tokens.in, locale)} / ${tokensText(s.tokens.out, locale)}`}
                note={t("conversations.usage.cached", {
                  cached: tokensText(s.tokens.cached, locale),
                  share: shareText(s.tokens.cachedShare, locale),
                })}
              />
            ) : null}
            <Figure label={t("conversations.usage.total")} value={msText(s.totalMs.p50)} note={spread(s.totalMs)} />
            <Figure
              label={t("conversations.usage.failures")}
              value={`${s.timeouts} / ${s.errors}`}
              note={t("conversations.usage.failureShare", {
                timeouts: percent(s.timeouts, s.questions),
                errors: percent(s.errors, s.questions),
              })}
            />
          </dl>
          {timed(s) ? (
            <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-5">
              {measured(s.firstTokenMs) ? (
                <Figure label={t("conversations.usage.firstToken")} value={msText(s.firstTokenMs.p50)} note={spread(s.firstTokenMs)} />
              ) : null}
              {s.answerCache && s.answerCache.hits > 0 ? (
                <Figure
                  label={t("conversations.usage.answerCache")}
                  value={shareText(s.answerCache.hitShare, locale)}
                  note={t("conversations.usage.answerCacheNote", {
                    hits: s.answerCache.hits,
                    ms: msText(s.answerCache.savedMs),
                    usd: usdText(s.answerCache.savedUsd),
                  })}
                />
              ) : null}
              {PHASES.filter((phase) => measured(s.phases[phase])).map((phase) => (
                <Figure
                  key={phase}
                  label={t(`conversations.usage.phase.${phase}`)}
                  value={msText(s.phases[phase].p50)}
                  note={spread(s.phases[phase])}
                />
              ))}
            </dl>
          ) : null}
          {priced(s) && s.costByStage.length > 0 ? (
            <div>
              <h3 className="text-caption font-semibold">{t("conversations.usage.byStage")}</h3>
              <ul className="flex flex-wrap gap-x-4 gap-y-1 text-caption">
                {s.costByStage.map((stage) => (
                  <li key={stage.stage}>
                    {stage.stage}: {costText(stage.costUsd, stage.uncostedCalls, unknown)} ·{" "}
                    {t("conversations.usage.calls", { count: stage.calls })}
                    {stage.uncostedCalls > 0 && stage.costUsd
                      ? ` (${t("conversations.uncosted", { count: stage.uncostedCalls })})`
                      : ""}
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
          {s.days.length > 0 ? (
            <div>
              <h3 className="text-caption font-semibold">{t("conversations.usage.days")}</h3>
              <ul className="grid grid-cols-1 gap-2 sm:grid-cols-2 lg:grid-cols-3">
                {s.days.map((day) => (
                  <li key={day.day} className="rounded border border-border p-2 text-caption">
                    <span className="font-semibold">{day.day}</span>{" "}
                    {t("conversations.usage.day", {
                      questions: day.questions,
                      cost: costText(day.costUsd, day.uncostedQuestions, unknown),
                      p50: msText(day.p50TotalMs),
                      timeouts: day.timeouts,
                      errors: day.errors,
                    })}
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
        </>
      ) : null}
    </Card>
  );
}
