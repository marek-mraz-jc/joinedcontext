/**
 * What a typed value has to be before it is sent (T-3097): the attribute's rule as its model states
 * it (LinkML range, `minimum_value`, `maximum_value`, `pattern`, `required`), checked at the cell so
 * a person sees what is wrong where they typed it. The gateway still decides every write; this
 * only saves the round trip for what the model already says.
 */

export interface ValueRule {
  kind?: "string" | "integer" | "number" | "boolean" | "datetime" | "date" | "uri";
  minimum?: number;
  maximum?: number;
  pattern?: string;
  required?: boolean;
}

export interface RuleLabels {
  mustBeInteger: string;
  mustBeNumber: string;
  mustBeBoolean: string;
  mustBeDate: string;
  mustBeUri: string;
  atLeast: string;
  atMost: string;
  patternMismatch: string;
  required: string;
}

/** What is wrong with `value` by `rule`, in the person's words, or nothing. */
export function problemOf(rule: ValueRule | undefined, value: unknown, labels: RuleLabels): string | null {
  if (!rule) return null;
  const text = value === undefined || value === null ? "" : String(value).trim();
  if (text === "") {
    return rule.required ? labels.required : null;
  }
  const numeric = rule.kind === "integer" || rule.kind === "number";
  if (numeric) {
    const n = Number(text);
    if (!Number.isFinite(n)) return labels.mustBeNumber;
    if (rule.kind === "integer" && !Number.isInteger(n)) return labels.mustBeInteger;
    if (rule.minimum !== undefined && n < rule.minimum) return `${labels.atLeast} ${rule.minimum}`;
    if (rule.maximum !== undefined && n > rule.maximum) return `${labels.atMost} ${rule.maximum}`;
  }
  if (rule.kind === "boolean" && text !== "true" && text !== "false") return labels.mustBeBoolean;
  if ((rule.kind === "datetime" || rule.kind === "date") && Number.isNaN(Date.parse(text))) return labels.mustBeDate;
  if (rule.kind === "uri" && !isUri(text)) return labels.mustBeUri;
  if (rule.pattern !== undefined) {
    let pattern: RegExp | null = null;
    try {
      pattern = new RegExp(rule.pattern, "u");
    } catch {
      // A pattern this browser cannot compile is the gateway's to check, not a reason to refuse.
      pattern = null;
    }
    if (pattern && !pattern.test(text)) return labels.patternMismatch;
  }
  return null;
}

function isUri(text: string): boolean {
  try {
    new URL(text);
    return true;
  } catch {
    return false;
  }
}
