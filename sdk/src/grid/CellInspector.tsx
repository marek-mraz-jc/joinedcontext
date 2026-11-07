import React, { useEffect, useId, useRef, useState } from "react";
import type { AttributeAccess } from "./source";

/** The Policy behind a refused attribute, where the host can name it (API/01 §58). */
export interface AccessExplanation {
  policy: string | null;
  policyTitle?: string | null;
  /** A Policy decided and the person may not read it. */
  policyHidden: boolean;
}

export type AccessAction = "read" | "write" | "delete";

export interface CellInspectorLabels {
  /** The panel's name before the attribute's: "Your access". */
  title: string;
  close: string;
  read: string;
  write: string;
  delete: string;
  allowed: string;
  notAllowed: string;
  loading: string;
  /** `{reason}` is the source's own sentence. */
  failed: string;
  grant: string;
  notInGrant: string;
  noGrant: string;
  formula: string;
  prohibited: string;
  /** `{policy}` is the Policy's title, else its name. */
  prohibitedBy: string;
  prohibitedHidden: string;
  naming: string;
  namingFailed: string;
  askAccess: string;
}

export const DEFAULT_INSPECTOR_LABELS: CellInspectorLabels = {
  title: "Your access",
  close: "Close",
  read: "Read",
  write: "Write",
  delete: "Delete",
  allowed: "allowed",
  notAllowed: "not allowed",
  loading: "Checking your access…",
  failed: "Your access could not be read ({reason}). Reload the page to try again.",
  grant: "Your access allows this.",
  notInGrant: "Not in your access.",
  noGrant: "Your access does not cover this type.",
  formula: "Calculated by a pipeline, so it is not edited here.",
  prohibited: "Explicitly not allowed by a policy.",
  prohibitedBy: "Explicitly not allowed by policy “{policy}”.",
  prohibitedHidden: "Hidden by a policy you cannot read.",
  naming: "Looking up the policy…",
  namingFailed: "The policy could not be named; try again shortly.",
  askAccess: "Ask for access",

};

export interface CellInspectorProps {
  /** The attribute's name, and the label its column shows. */
  attribute: string;
  label: string;
  /** The decisions; `undefined` while they are read. */
  access: AttributeAccess | undefined;
  /** Why the decisions could not be read, where they could not. */
  failed?: string;
  /** Names the Policy behind a prohibition; asked only when the panel shows one. */
  explain?: (attribute: string, action: AccessAction) => Promise<AccessExplanation>;
  /** Where the person asks for access, offered when the attribute is outside their grant. */
  askAccessHref?: string;
  labels?: Partial<CellInspectorLabels>;
  onClose: () => void;
}

const ACTIONS: readonly AccessAction[] = ["read", "write", "delete"];

const fill = (text: string, name: string, value: string) => text.replace(`{${name}}`, value);

/**
 * What the person may do with one attribute, and why (T-5528): a chip per action in its own
 * colour (read green, write blue, delete amber; grey with a lock when refused), the reason in
 * words, and a way to ask for access. A side panel on a wide screen, a bottom sheet on a phone. It
 * renders the server's decisions only; the Endpoint still decides every request.
 */
export function CellInspector({ attribute, label, access, failed, explain, askAccessHref, labels: partial, onClose }: CellInspectorProps): React.JSX.Element {
  const labels = { ...DEFAULT_INSPECTOR_LABELS, ...partial };
  const headingId = useId();
  const heading = useRef<HTMLHeadingElement>(null);
  const refused = access ? ACTIONS.find((action) => !access[action]) : undefined;
  const asking = access?.reason === "prohibited" && refused !== undefined && explain !== undefined;
  const [explained, setExplained] = useState<AccessExplanation | null | undefined>(undefined);

  useEffect(() => {
    heading.current?.focus();
  }, [attribute]);
  useEffect(() => {
    if (!asking || !refused || !explain) return;
    let current = true;
    setExplained(undefined);
    explain(attribute, refused).then(
      (answer) => current && setExplained(answer),
      () => current && setExplained(null),
    );
    return () => {
      current = false;
    };
  }, [asking, attribute, refused, explain]);

  const reason = (): string => {
    if (!access) return "";
    switch (access.reason) {
      case "grant":
        return labels.grant;
      case "not_in_grant":
        return labels.notInGrant;
      case "no_grant":
        return labels.noGrant;
      case "formula":
        return labels.formula;
      case "prohibited":
        if (!asking) return labels.prohibited;
        if (explained === undefined) return labels.naming;
        if (explained === null) return `${labels.prohibited} ${labels.namingFailed}`;
        if (explained.policy) return fill(labels.prohibitedBy, "policy", explained.policyTitle || explained.policy);
        return explained.policyHidden ? labels.prohibitedHidden : labels.prohibited;
    }
  };
  const ask = access && (access.reason === "not_in_grant" || access.reason === "no_grant") && askAccessHref;

  return (
    <aside
      className="jc-grid-inspector"
      role="dialog"
      aria-modal="false"
      aria-labelledby={headingId}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          onClose();
        }
      }}
    >
      <div className="jc-grid-inspector-head">
        <h3 id={headingId} ref={heading} tabIndex={-1}>
          {labels.title}: {label}
        </h3>
        <button type="button" className="jc-grid-inspector-close" onClick={onClose} aria-label={labels.close}>
          ×
        </button>
      </div>
      {failed !== undefined ? (
        <p role="alert" className="jc-grid-inspector-failed">
          {fill(labels.failed, "reason", failed)}
        </p>
      ) : access === undefined ? (
        <p role="status">{labels.loading}</p>
      ) : (
        <>
          <ul className="jc-grid-chips" aria-label={labels.title}>
            {ACTIONS.map((action) => (
              <li key={action} className={`jc-grid-chip jc-grid-chip--${access[action] ? action : "none"}`}>
                {access[action] ? null : <LockIcon />}
                {labels[action]}: {access[action] ? labels.allowed : labels.notAllowed}
              </li>
            ))}
          </ul>
          <p role="status" className="jc-grid-inspector-reason">
            {reason()}
          </p>
          {ask ? (
            <a className="jc-grid-inspector-ask" href={askAccessHref}>
              {labels.askAccess}
            </a>
          ) : null}
        </>
      )}
    </aside>
  );
}

function LockIcon(): React.JSX.Element {
  return (
    <svg aria-hidden="true" focusable="false" width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
      <path d="M8 1a3.5 3.5 0 0 0-3.5 3.5V7H4a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1V8a1 1 0 0 0-1-1h-.5V4.5A3.5 3.5 0 0 0 8 1Zm2 6H6V4.5a2 2 0 1 1 4 0V7Z" />
    </svg>
  );
}
