import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "../../api/schema";
import { Badge, Card, CardHeader } from "../ui";
import type { BadgeTone } from "../ui/Badge";

type Score = components["schemas"]["DatasetScore"];
type Said = components["schemas"]["DatasetScoreText"];

/** The message keys the server sends (`SAY` in `src/dataset_score.rs`), each under `quality.score.say`. */
export const SAID = [
  "notMeasured", "noSchedule", "onTime", "late", "lateUnread", "noRequired", "complete", "valid", "linksUp",
  "fixLate", "fixFailing", "fixSilent", "fixMissing", "fixInvalid", "fixLink",
] as const;

const ISO = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}/;

/** 80 and up reads as good, under 50 as poor; the number and its word carry it, not the colour. */
export function toneOf(score: number | null | undefined): BadgeTone {
  if (score == null) return "neutral";
  if (score >= 80) return "success";
  return score >= 50 ? "warning" : "danger";
}

function word(score: number | null | undefined): string {
  if (score == null) return "quality.score.unmeasured";
  if (score >= 80) return "quality.score.good";
  return score >= 50 ? "quality.score.fair" : "quality.score.poor";
}

/**
 * A dataset's quality score (T-5112): the badge with its number, the date it was measured, and
 * each of its four parts with the reason, plus what to fix when the reader is its publisher.
 */
export function QualityScore({ score, id = "dataset-quality" }: { score: Score; id?: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const when = (value: string | null | undefined) => {
    const parsed = value ? new Date(value) : null;
    return parsed && !Number.isNaN(parsed.getTime())
      ? parsed.toLocaleString(locale, { dateStyle: "medium", timeStyle: "short" })
      : null;
  };
  // a time the server names reads as a date in the reader's language, never as RFC 3339
  const say = (text: Said) =>
    (SAID as readonly string[]).includes(text.code)
      ? t(`quality.score.say.${text.code}`, {
          ...Object.fromEntries(
            Object.entries(text.args).map(([key, value]) => [key, ISO.test(value) ? (when(value) ?? value) : value]),
          ),
          defaultValue: text.text,
        })
      : text.text;
  const measured = when(score.measuredAt);
  return (
    <Card as="section" aria-labelledby={id} data-testid="dataset-quality">
      <CardHeader
        titleId={id}
        title={t("quality.score.title")}
        description={measured ? t("quality.score.measured", { when: measured }) : t("quality.score.notYet")}
        actions={
          <Badge tone={toneOf(score.score)} data-testid="quality-badge">
            {score.score == null
              ? t("quality.score.unmeasured")
              : t("quality.score.badge", { score: score.score, word: t(word(score.score)) })}
          </Badge>
        }
      />
      <ul className="mt-4 grid gap-3 sm:grid-cols-2">
        {score.parts.map((part) => (
          <li key={part.part} className="flex min-w-0 flex-col gap-1 rounded-lg border border-border p-3">
            <div className="flex items-center justify-between gap-2">
              <span className="text-body font-semibold text-fg">{t(`quality.score.part.${part.part}`)}</span>
              <span className="text-body tabular-nums text-fg">
                {part.score == null ? t("quality.score.dash") : t("quality.score.of100", { score: part.score })}
              </span>
            </div>
            <meter
              aria-label={t(`quality.score.part.${part.part}`)}
              aria-valuetext={part.score == null ? t("quality.score.unmeasured") : t("quality.score.of100", { score: part.score })}
              min={0}
              max={100}
              low={50}
              high={80}
              optimum={100}
              value={part.score ?? 0}
              className="h-2 w-full"
            />
            <p className="text-caption text-fg-muted [overflow-wrap:anywhere]">{say(part.reason)}</p>
            {part.fix ? (
              <p className="text-caption text-fg [overflow-wrap:anywhere]">
                <span className="font-semibold">{t("quality.score.toFix")}</span> {say(part.fix)}
              </p>
            ) : null}
          </li>
        ))}
      </ul>
      <p className="mt-3 text-caption text-fg-muted">{t("quality.score.hint")}</p>
    </Card>
  );
}
