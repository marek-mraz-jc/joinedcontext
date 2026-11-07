import type { JsonSchema } from "../components/forms/types";
import { storedMetadata } from "../api/manifest";
import { DID_WEB_PATTERN } from "./dataagreement";
import { DNS1123, words } from "./kinds";

/**
 * The DataSpaceParticipant form (T-1544, DS-04, DS-06, DS-07): how this organization takes part in a
 * data space, written once per organization on the Organization page.
 *
 * jc-core keeps one participant per organization (`dataspace/participant.yaml`), so the page offers
 * New only while there is none. Both URLs are HTTPS, which the field and jc-core both check.
 */

export const CONNECTOR_ENGINES = ["rainbow", "edc"] as const;

/** An HTTPS URL, as jc-core asks of the issuer and the connector (DS-07). */
export const HTTPS_PATTERN = "^https://\\S+$";

export interface DataSpaceParticipantForm {
  name?: string;
  did?: string;
  credentialIssuer?: string;
  connectorUrl?: string;
  engine?: string;
  trustAnchors?: string[];
}

export function dataSpaceParticipantSchema(t: (key: string) => string): JsonSchema {
  const text = (key: string): JsonSchema => ({ type: "string", title: t(`dataspaceparticipants.field.${key}`) });
  return {
    type: "object",
    required: ["name", "did", "credentialIssuer", "connectorUrl", "engine"],
    properties: {
      name: { ...text("name"), pattern: DNS1123, maxLength: 63 },
      did: { ...text("did"), pattern: DID_WEB_PATTERN, maxLength: 261 },
      credentialIssuer: { ...text("credentialIssuer"), pattern: HTTPS_PATTERN },
      connectorUrl: { ...text("connectorUrl"), pattern: HTTPS_PATTERN },
      engine: { ...text("engine"), ...words(t, "choice.connectorEngine", CONNECTOR_ENGINES), default: "rainbow" },
      trustAnchors: {
        type: "array",
        title: t("dataspaceparticipants.field.trustAnchors"),
        uniqueItems: true,
        items: { ...text("trustAnchor"), pattern: DID_WEB_PATTERN, maxLength: 261 },
      },
    },
  };
}

const filled = (value: string | undefined): string | undefined =>
  value && value.trim() !== "" ? value.trim() : undefined;

/** The form as the manifest the API stores; what the form has no field for travels on from `stored`. */
export function toDataSpaceParticipantManifest(form: DataSpaceParticipantForm, stored?: unknown): unknown {
  const anchors = (form.trustAnchors ?? []).flatMap((anchor) => filled(anchor) ?? []);
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataSpaceParticipant",
    metadata: { ...storedMetadata(stored), name: form.name, namespace: "org" },
    spec: {
      did: filled(form.did),
      credentialIssuer: filled(form.credentialIssuer),
      connectorUrl: filled(form.connectorUrl),
      engine: form.engine,
      ...(anchors.length > 0 ? { trustAnchors: anchors } : {}),
    },
  };
}

/** The stored manifest back as the form. */
export function fromDataSpaceParticipantManifest(manifest: unknown): DataSpaceParticipantForm {
  const envelope = (manifest ?? {}) as { metadata?: { name?: string }; spec?: Record<string, unknown> };
  const spec = envelope.spec ?? {};
  const text = (key: string) => (typeof spec[key] === "string" ? { [key]: spec[key] as string } : {});
  return {
    name: envelope.metadata?.name ?? "",
    ...text("did"),
    ...text("credentialIssuer"),
    ...text("connectorUrl"),
    ...text("engine"),
    trustAnchors: Array.isArray(spec.trustAnchors) ? (spec.trustAnchors as string[]) : [],
  };
}
