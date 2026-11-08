import { useEffect, useState } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Alert, Badge, Button } from "../ui";
import type { BadgeTone } from "../ui";
import { ago } from "../ago";
import type { Verdict } from "../../api/drafts";

export type VerdictState = "none" | "green" | "waiting" | "red" | "stale";

/**
 * The chip's tone per verdict; the words say the same, so the colour is never alone (UI-30).
 * `waiting` passes and names a resource an open change creates (MF-48): proposable, and its change
 * merges after that one.
 */
const VERDICT_TONE: Record<VerdictState, BadgeTone> = {
  none: "neutral",
  green: "success",
  waiting: "info",
  red: "danger",
  stale: "warning",
};

/** What a verdict says of the manifest the form holds: fresh only for the digest it judged. */
export function verdictStateOf(verdict: Verdict | null, digest: string): VerdictState {
  if (!verdict) return "none";
  if (verdict.inputDigest !== digest) return "stale";
  if (!verdict.ok) return "red";
  return (verdict.waitsOn ?? []).length > 0 ? "waiting" : "green";
}

/** The verdict chip, its age read against a clock that moves while it is on screen. */
function VerdictChip({
  verdict,
  state,
  refused,
}: {
  verdict: Verdict | null;
  state: VerdictState;
  refused: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  // "Checked 0 s ago" does not stay that for as long as the dialog is open.
  const [now, setNow] = useState(() => Date.now());
  const hasVerdict = verdict !== null;
  useEffect(() => {
    if (!hasVerdict) {
      return undefined;
    }
    const timer = setInterval(() => setNow(Date.now()), 15_000);
    return () => clearInterval(timer);
  }, [hasVerdict]);
  // The age in the person's language: the verdict sentence is translated, and an English
  // "12s ago" inside a German one was half a translation (T-1753).
  const age = verdict?.checkedAt ? ago(verdict.checkedAt, now, t) : "";
  return (
    <Badge data-testid="draft-verdict" tone={refused ? "danger" : VERDICT_TONE[state]}>
      {state === "none" && (refused ? t("drafts.verdict.refused") : t("drafts.verdict.none"))}
      {state === "green" && t("drafts.verdict.green", { age })}
      {state === "waiting" && t("drafts.verdict.waiting", { changes: (verdict?.waitsOn ?? []).join(", "), age })}
      {state === "red" && t("drafts.verdict.red", { age })}
      {state === "stale" && t("drafts.verdict.stale")}
    </Badge>
  );
}

/**
 * "Discard the draft": what the person typed is shared with their other windows and the assistant,
 * so it goes only after a second, named confirmation, and a refusal is said beside the button.
 */
export function DiscardDraft({ onDiscard }: { onDiscard: () => Promise<void> }): JSX.Element {
  const { t } = useTranslation();
  const [asking, setAsking] = useState(false);
  const [pending, setPending] = useState(false);
  const [refused, setRefused] = useState<string | null>(null);
  async function discard() {
    setPending(true);
    setRefused(null);
    try {
      await onDiscard();
      setAsking(false);
    } catch (err) {
      const detail = (err as { problem?: { detail?: unknown } } | null)?.problem?.detail;
      setRefused(typeof detail === "string" && detail ? detail : err instanceof Error ? err.message : t("app.error.generic"));
    } finally {
      setPending(false);
    }
  }
  if (!asking) {
    return (
      <Button variant="ghost" size="sm" onClick={() => setAsking(true)}>
        {t("drafts.discard.ask")}
      </Button>
    );
  }
  return (
    // A line of its own at the footer's end, so the question is never read as the answer to the
    // Cancel beside it, and its refusal is never mistaken for the check's.
    <div role="group" aria-label={t("drafts.discard.ask")} className="order-last flex basis-full flex-col items-end gap-2">
      <div className="flex flex-wrap items-center justify-end gap-2">
        <span className="text-body text-fg">{t("drafts.discard.confirm")}</span>
        <Button
          variant="danger"
          size="sm"
          loading={pending}
          // A refused draft stays refused until something changes on the server: say so, once.
          disabled={refused !== null}
          disabledReason={refused ?? undefined}
          onClick={() => void discard()}
        >
          {t("drafts.discard.yes")}
        </Button>
        <Button variant="secondary" size="sm" disabled={pending} disabledReason={t("app.busyReason")} onClick={() => setAsking(false)}>
          {t("drafts.discard.keep")}
        </Button>
      </div>
      {refused ? (
        <Alert tone="danger" role="alert">
          {t("drafts.discard.refused", { reason: refused })}
        </Alert>
      ) : null}
    </div>
  );
}

export interface ResourceFormFooterProps {
  onCancel: () => void;
  /** The Check button, when the form can be checked at all. */
  check?: { onClick: () => void; disabled: boolean; disabledReason: string };
  /** Why the last Check stopped before asking the server. */
  stoppedMessage: string | null;
  /** The verdict chip shows when the form has a draft to judge. */
  verdict?: { verdict: Verdict | null; state: VerdictState; refused: boolean };
  /** Why Propose waits for a check, and whether lax validation only warns of it. */
  proposeReason?: string;
  lax: boolean;
  /** What the footer's buttons answered, beside them, where the person is looking (T-1424). */
  error: string | null;
  className?: string;
  /** Throws the saved draft away, when the form has one (AG-61): asked once more before it goes. */
  onDiscard?: () => Promise<void>;
  /** The YAML view's footer holds Propose too, so tests scope to it; the form view's footer sits inside theme.tsx's. */
  testId?: string;
  children?: ReactNode;
}

/** The line under a form or its YAML: Cancel, Check, the verdict and what each answered. */
export function ResourceFormFooter({
  onCancel,
  check,
  stoppedMessage,
  verdict,
  proposeReason,
  lax,
  error,
  onDiscard,
  className = "",
  testId,
  children,
}: ResourceFormFooterProps): JSX.Element {
  const { t } = useTranslation();
  return (
    <div data-testid={testId} className={`flex flex-wrap items-center gap-2 ${className}`.trim()}>
      <Button variant="ghost" onClick={onCancel}>
        {t("form.cancel")}
      </Button>
      {onDiscard ? <DiscardDraft onDiscard={onDiscard} /> : null}
      {check ? (
        <Button size="sm" disabled={check.disabled} disabledReason={check.disabledReason} onClick={check.onClick}>
          {t("form.check")}
        </Button>
      ) : null}
      {stoppedMessage !== null ? (
        <span data-testid="check-stopped" role="status" className="text-caption text-danger">
          {stoppedMessage}
        </span>
      ) : null}
      {verdict ? (
        <>
          <VerdictChip {...verdict} />
          {proposeReason ? (
            <span
              id="propose-reason"
              data-testid="propose-reason"
              role="status"
              // Before any check nothing is wrong yet: the reason is a next step, not an error.
              className={`text-caption ${lax ? "text-warning" : verdict.state === "none" && !verdict.refused ? "text-fg-muted" : "text-danger"}`}
            >
              {lax ? t("drafts.laxWarning") : proposeReason}
            </span>
          ) : null}
        </>
      ) : null}
      {error ? (
        <span data-testid="footer-error" role="alert" className="basis-full text-right text-caption text-danger">
          {error}
        </span>
      ) : null}
      {children}
    </div>
  );
}
