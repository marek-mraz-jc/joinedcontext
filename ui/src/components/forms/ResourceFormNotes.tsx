import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Alert, Button, ExternalLink, Icon, safeHref } from "../ui";
import type { Draft, Verdict } from "../../api/drafts";
import { askAbout } from "../../assistant/state";
import { kindPlural } from "../../kindWords";
import { findingSentence } from "./resourceForm";

const FINDING_TONE: Record<string, string> = { error: "text-danger", warning: "text-warning" };

/** Every finding of the verdict, with its path: one with no field to sit on is read here. */
function Findings({ findings }: { findings: NonNullable<Verdict["findings"]> }): JSX.Element | null {
  const { t } = useTranslation();
  if (findings.length === 0) return null;
  return (
    <div
      data-testid="draft-findings"
      className="flex flex-col gap-1 rounded border border-border bg-surface-subtle p-2 text-caption"
    >
      <span className="font-semibold text-fg">{t("drafts.findings")}</span>
      <ul className="list-disc pl-4 space-y-0.5">
        {findings.map((f, idx) => (
          <li key={idx} className={FINDING_TONE[f.level] ?? "text-fg-muted"}>
            {f.path ? <span className="font-mono">{f.path}: </span> : null}
            <span>{findingSentence(f.message, t)}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

export interface ResourceFormNotesProps {
  conflict: string | null;
  saveRefused: string | null;
  project?: string;
  draftKind?: string;
  draft: Draft | null;
  verdict: Verdict | null;
  restartsStream: boolean;
}

/**
 * The kind's page in the User Guide as a "?" beside the form's title, and only when this
 * installation serves one (UI-02, DP-11, T-2252, T-6105): the form opens on its first field, with
 * no sentence above it. The label names the page, so a screen reader announces where it goes.
 *
 * The base half of the address is the installation's `documentationBaseUrl`, which arrives from a
 * manifest: an address that is not a place renders nothing (PF-50, T-2409), and `ExternalLink`
 * carries `rel="noopener noreferrer"`, which the form behind it needs because it may hold typing.
 */
export function FormGuideLink({ kind, href }: { kind?: string; href?: string | null }): JSX.Element | null {
  const { t } = useTranslation();
  if (!safeHref(href)) return null;
  const label = `${t("form.guideLink", { kind: kind ? kindPlural(kind, t) : "" })} (${t("app.opensNewTab")})`;
  return (
    <ExternalLink
      data-testid="form-guide"
      href={href}
      hideIcon
      aria-label={label}
      title={label}
      className="inline-flex size-control shrink-0 items-center justify-center rounded-md text-fg-muted no-underline hover:bg-surface-muted hover:text-fg"
    >
      <Icon name="help" className="size-4" aria-hidden="true" />
    </ExternalLink>
  );
}

/**
 * "Fill this from a sentence": opens the assistant with the form's kind in the question. It sits on
 * the row of the Form/YAML tabs, so the first field stays in the first screen at 400 px (T-6163).
 */
export function FillFromSentence({ kind }: { kind: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <Button
      type="button"
      variant="ghost"
      size="sm"
      onClick={() => askAbout(t("form.fillFromSentenceQuestion", { kind }))}
    >
      {t("form.fillFromSentence")}
    </Button>
  );
}

/** What a form says above its fields: the draft's state, and only what changes what happens. */
export function ResourceFormNotes(props: ResourceFormNotesProps): JSX.Element {
  const { t } = useTranslation();
  const { draftKind } = props;
  return (
    <>
      {props.conflict ? (
        <Alert role="alert" tone="warning">
          {props.conflict}
        </Alert>
      ) : null}
      {props.saveRefused ? (
        <Alert role="alert" tone="danger">
          {t("drafts.saveRefused", { reason: props.saveRefused })}
        </Alert>
      ) : null}

      {draftKind && props.verdict?.findings ? <Findings findings={props.verdict.findings} /> : null}

      {props.restartsStream ? (
        <Alert role="status" tone="warning">
          {t("form.restartsStream")}
        </Alert>
      ) : null}
    </>
  );
}
