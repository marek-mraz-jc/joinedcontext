/**
 * What an App lacks before it is published (T-3267, AP-140): each item is fine, a warning that
 * publishing may go on with, or a privacy problem that holds it. A warning names where it is
 * fixed; a problem names why the data would reach someone it must not.
 */
import type { Manifest } from "../../api/manifest";
import { refName } from "../../api/manifest";
import { grantChanges } from "../../components/endpoints/operationGroups";
import type { AppCheck } from "./AppCheckChip";

export type ItemState = "ok" | "warning" | "problem";
export type ItemKey = "title" | "description" | "licence" | "visibility" | "checks" | "contact" | "privacy";

export interface ChecklistItem {
  key: ItemKey;
  state: ItemState;
  /** What the item found, for the sentence the dialog shows. */
  detail?: string;
  /** Why the data would reach someone it must not: a privacy problem's cause. */
  reason?: "writes" | "unpublished" | "personal";
}

/** Where an App's licence and contact are written, beside its title (AP-140). */
export const LICENCE_ANNOTATION = "joinedcontext.com/licence";
export const CONTACT_ANNOTATION = "joinedcontext.com/contact";

/**
 * Attribute names that are a person's own data. ponytail: a name list, not a model property; a
 * model that marks personal slots (MIM4-R4) replaces it.
 */
const PERSONAL = /^(e-?mail(address)?|telephone|phone(number)?|mobile|birth(date|day)?|dateofbirth|nationalid|personalid|ssn|iban|passport(number)?|givenname|familyname|firstname|lastname)$/i;

interface Need {
  contextSpaceRef?: unknown;
  attrs?: unknown;
  operations?: unknown;
}

const text = (value: unknown): string => (typeof value === "string" ? value.trim() : "");
const titled = (value: unknown): boolean =>
  typeof value === "string"
    ? value.trim() !== ""
    : typeof value === "object" && value !== null && Object.values(value).some((v) => text(v) !== "");

export function publishChecklist(
  app: Manifest,
  context: {
    /** The installation's defaults (branding). */
    licenceDefault?: string;
    contactEmail?: string;
    /** The probe's last verdict on this App, if one ran (AP-136). */
    check?: AppCheck;
    /** The project's Endpoints, to tell a space published to the public from one that is not. */
    endpoints: Manifest[];
  },
): ChecklistItem[] {
  const annotations = app.metadata.annotations ?? {};
  const spec = app.spec as { visibility?: string; dataNeeds?: Need[] };
  const visibility = spec.visibility ?? "project";
  const licence = text(annotations[LICENCE_ANNOTATION]) || text(context.licenceDefault);
  const contact = text(annotations[CONTACT_ANNOTATION]) || text(context.contactEmail);
  const items: ChecklistItem[] = [
    { key: "title", state: titled(app.metadata.title) ? "ok" : "warning" },
    { key: "description", state: titled(app.metadata.description) ? "ok" : "warning" },
    { key: "licence", state: licence ? "ok" : "warning", detail: licence || undefined },
    // A private App is no App anyone opens: jc-core refuses to publish one.
    { key: "visibility", state: visibility === "private" ? "problem" : "ok", detail: visibility },
    {
      key: "checks",
      state: context.check?.state === "green" ? "ok" : "warning",
      detail: context.check?.state === "red" ? (context.check.reason ?? undefined) : undefined,
    },
    { key: "contact", state: contact ? "ok" : "warning", detail: contact || undefined },
  ];
  if (visibility === "public") {
    const publicSpaces = new Set(
      context.endpoints
        .filter((endpoint) => (endpoint.spec as { audience?: unknown }).audience === "public")
        .map((endpoint) => refName((endpoint.spec as { contextSpaceRef?: unknown }).contextSpaceRef))
        .filter((space): space is string => Boolean(space)),
    );
    const problems: ChecklistItem[] = [];
    for (const need of spec.dataNeeds ?? []) {
      const space = refName(need.contextSpaceRef) ?? "";
      const operations = Array.isArray(need.operations) ? need.operations.filter((op): op is string => typeof op === "string") : [];
      const attrs = Array.isArray(need.attrs) ? need.attrs.filter((a): a is string => typeof a === "string") : [];
      // A public app must not write: jc-core's own list of writing operations and groups decides,
      // not a name prefix (F-UI2-015).
      if (grantChanges(operations) !== "nothing") problems.push({ key: "privacy", state: "problem", reason: "writes", detail: space });
      if (space && !publicSpaces.has(space)) problems.push({ key: "privacy", state: "problem", reason: "unpublished", detail: space });
      for (const attr of attrs.filter((a) => PERSONAL.test(a))) {
        problems.push({ key: "privacy", state: "problem", reason: "personal", detail: attr });
      }
    }
    items.push(...(problems.length > 0 ? problems : [{ key: "privacy" as const, state: "ok" as const }]));
  }
  return items;
}

/** Whether publishing may go on: warnings may stay, a problem may not. */
export function mayPublish(items: ChecklistItem[]): boolean {
  return items.every((item) => item.state !== "problem");
}
