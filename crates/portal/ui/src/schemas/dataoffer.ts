import type { JsonSchema, UiSchema } from "../components/forms/types";
import { storedMetadata } from "../api/manifest";
import { DNS1123 } from "./kinds";
import { SPACE_LABEL } from "./modelprojection";

/**
 * The DataOffer form (T-1543, DS-07, DS-08, DS-14): the endpoints of one context space this project
 * offers to the data space, and the ODRL 2.2 offer they are offered under.
 *
 * The offer is ODRL written as JSON in a text area and stored as the object it parses to; text that
 * does not parse travels as written, and jc-core refuses it naming `spec.policy`. Personal data asks
 * for a purpose from the Data Privacy Vocabulary, which jc-core also checks.
 */

/** A purpose IRI of the Data Privacy Vocabulary (DS-14). */
export const DPV_PATTERN = "^https://w3id\\.org/dpv";

/** The smallest offer jc-core accepts: one permission, to use. */
export const OPEN_OFFER = JSON.stringify({ "@type": "Offer", permission: [{ action: "use" }] }, null, 2);

export interface DataOfferForm {
  name?: string;
  contextSpaceRef?: string;
  endpointRefs?: string[];
  policy?: string;
  containsPersonalData?: boolean;
  purpose?: string;
}

export function dataOfferSchema(t: (key: string) => string): JsonSchema {
  const text = (key: string): JsonSchema => ({ type: "string", title: t(`dataoffers.field.${key}`) });
  return {
    type: "object",
    required: ["name", "contextSpaceRef", "endpointRefs", "policy"],
    properties: {
      name: { ...text("name"), pattern: DNS1123, maxLength: 63 },
      contextSpaceRef: { ...text("contextSpaceRef"), pattern: DNS1123, maxLength: 63 },
      // DS-07: an offer of nothing is not an offer.
      endpointRefs: {
        type: "array",
        title: t("dataoffers.field.endpointRefs"),
        minItems: 1,
        uniqueItems: true,
        items: { ...text("endpoint"), pattern: DNS1123, maxLength: 63 },
      },
      policy: { ...text("policy"), pattern: "\\S", default: OPEN_OFFER },
      containsPersonalData: { type: "boolean", title: t("dataoffers.field.containsPersonalData"), default: false },
      purpose: { ...text("purpose"), pattern: DPV_PATTERN },
    },
  };
}

export const dataOfferUiSchema: UiSchema = {
  policy: { "ui:widget": "textarea", "ui:options": { rows: 10 }, "ui:autocomplete": "off" },
};

const filled = (value: string | undefined): string | undefined =>
  value && value.trim() !== "" ? value.trim() : undefined;

/** The offer as the object it parses to, or the text as written when it does not parse. */
function parsed(text: string | undefined): unknown {
  if (text === undefined) return undefined;
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return text;
  }
}

/** The form as the manifest the API stores; what the form has no field for travels on from `stored`. */
export function toDataOfferManifest(project: string, form: DataOfferForm, stored?: unknown): unknown {
  const purpose = filled(form.purpose);
  const space = filled(form.contextSpaceRef);
  const metadata = storedMetadata(stored) as { labels?: Record<string, string> };
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataOffer",
    // The space as the label the repository path is derived from, so an offer is filed under its space.
    metadata: {
      ...metadata,
      name: form.name,
      namespace: project,
      // On create only: a label written on an edit would move the file and leave the old one behind.
      ...(space && stored === undefined ? { labels: { ...metadata.labels, [SPACE_LABEL]: space } } : {}),
    },
    spec: {
      contextSpaceRef: space,
      endpointRefs: (form.endpointRefs ?? []).flatMap((name) => {
        const endpoint = filled(name);
        return endpoint ? [{ kind: "Endpoint", name: endpoint }] : [];
      }),
      policy: parsed(form.policy),
      containsPersonalData: form.containsPersonalData ?? false,
      ...(purpose ? { purpose } : {}),
    },
  };
}

/** A reference as the form holds it: the name, whether the manifest wrote it bare or typed. */
function nameOf(reference: unknown): string | undefined {
  if (typeof reference === "string") return reference;
  const name = (reference as { name?: unknown } | null | undefined)?.name;
  return typeof name === "string" ? name : undefined;
}

/** The stored manifest back as the form, the offer as indented JSON. */
export function fromDataOfferManifest(manifest: unknown): DataOfferForm {
  const envelope = (manifest ?? {}) as { metadata?: { name?: string }; spec?: Record<string, unknown> };
  const spec = envelope.spec ?? {};
  return {
    name: envelope.metadata?.name ?? "",
    ...(typeof spec.contextSpaceRef === "string" ? { contextSpaceRef: spec.contextSpaceRef } : {}),
    endpointRefs: Array.isArray(spec.endpointRefs) ? spec.endpointRefs.flatMap((one) => nameOf(one) ?? []) : [],
    ...(spec.policy !== undefined
      ? { policy: typeof spec.policy === "string" ? spec.policy : JSON.stringify(spec.policy, null, 2) }
      : {}),
    containsPersonalData: spec.containsPersonalData === true,
    ...(typeof spec.purpose === "string" ? { purpose: spec.purpose } : {}),
  };
}
