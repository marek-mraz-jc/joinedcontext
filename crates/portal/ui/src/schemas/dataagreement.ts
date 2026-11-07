import type { JsonSchema } from "../components/forms/types";
import { storedMetadata } from "../api/manifest";
import { DNS1123, RFC3339_PATTERN, words } from "./kinds";

/**
 * The DataAgreement form (T-1542, DS-09, DS-10, DS-15): the record of one contract with another
 * participant of the data space, written by a person when the negotiation happened elsewhere.
 *
 * jc-core decides what a valid agreement is, and its refusals name the field: a provider without
 * its offer, a finalized consumer agreement without its token's secret, a validity that ends before
 * it starts. The form holds each field as the manifest does, so the YAML view and the fields agree.
 */

export const AGREEMENT_ROLES = ["provider", "consumer"] as const;

/** The negotiation states of DS-09, in the order a negotiation moves through them. */
export const AGREEMENT_STATES = ["requested", "offered", "accepted", "finalized", "terminated"] as const;

/** `did:web:` and a lowercase DNS name of at least two labels (jc-core `Did::new`, DS-04). */
export const DID_WEB_PATTERN =
  "^did:web:[a-z0-9]([-a-z0-9]*[a-z0-9])?(\\.[a-z0-9]([-a-z0-9]*[a-z0-9])?)+$";

export interface DataAgreementForm {
  name?: string;
  role?: string;
  offerRef?: string;
  remoteParticipant?: string;
  agreementId?: string;
  state?: string;
  validity?: { from?: string; to?: string };
  tokenSecretRef?: { name?: string; key?: string };
  constraints?: { q?: string; scopeQ?: string; geoQ?: string; temporalQ?: string };
}

const CONSTRAINTS = ["q", "scopeQ", "geoQ", "temporalQ"] as const;

/** The form's schema; `offers` are the project's DataOffers, offered as the provider's choice. */
export function dataAgreementSchema(t: (key: string) => string, offers: string[] = []): JsonSchema {
  const text = (key: string): JsonSchema => ({ type: "string", title: t(`dataagreements.field.${key}`) });
  return {
    type: "object",
    required: ["name", "role", "remoteParticipant", "agreementId", "state"],
    properties: {
      name: { ...text("name"), pattern: DNS1123, maxLength: 63 },
      role: { ...text("role"), ...words(t, "choice.agreementRole", AGREEMENT_ROLES), default: "consumer" },
      offerRef: {
        ...text("offerRef"),
        pattern: DNS1123,
        maxLength: 63,
        ...(offers.length > 0 ? { examples: offers } : {}),
      },
      remoteParticipant: { ...text("remoteParticipant"), pattern: DID_WEB_PATTERN, maxLength: 261 },
      // Non-empty after trimming, as jc-core refuses it (DS-09).
      agreementId: { ...text("agreementId"), pattern: "\\S" },
      state: { ...text("state"), ...words(t, "choice.agreementState", AGREEMENT_STATES), default: "requested" },
      validity: {
        type: "object",
        title: t("dataagreements.field.validity"),
        properties: {
          from: { ...text("validFrom"), pattern: RFC3339_PATTERN },
          to: { ...text("validTo"), pattern: RFC3339_PATTERN },
        },
      },
      tokenSecretRef: {
        type: "object",
        title: t("dataagreements.field.tokenSecretRef"),
        properties: {
          name: { ...text("secretName"), pattern: DNS1123, maxLength: 63 },
          key: text("secretKey"),
        },
      },
      constraints: {
        type: "object",
        title: t("dataagreements.field.constraints"),
        properties: Object.fromEntries(CONSTRAINTS.map((key) => [key, text(key)])),
      },
    },
  };
}

const filled = (value: string | undefined): string | undefined =>
  value && value.trim() !== "" ? value.trim() : undefined;

/** A reference as the form holds it: the name, whether the manifest wrote it bare or typed. */
function nameOf(reference: unknown): string | undefined {
  if (typeof reference === "string") return reference;
  const name = (reference as { name?: unknown } | null | undefined)?.name;
  return typeof name === "string" ? name : undefined;
}

/** The form as the manifest the API stores; what the form has no field for travels on from `stored`. */
export function toDataAgreementManifest(project: string, form: DataAgreementForm, stored?: unknown): unknown {
  const offer = filled(form.offerRef);
  const from = filled(form.validity?.from);
  const to = filled(form.validity?.to);
  const secret = filled(form.tokenSecretRef?.name);
  const key = filled(form.tokenSecretRef?.key);
  const constraints = Object.fromEntries(
    CONSTRAINTS.flatMap((name) => {
      const value = filled(form.constraints?.[name]);
      return value ? [[name, value]] : [];
    }),
  );
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataAgreement",
    metadata: { ...storedMetadata(stored), name: form.name, namespace: project },
    spec: {
      role: form.role,
      ...(offer ? { offerRef: { kind: "DataOffer", name: offer } } : {}),
      remoteParticipant: filled(form.remoteParticipant),
      agreementId: filled(form.agreementId),
      state: form.state,
      validity: { ...(from ? { from } : {}), ...(to ? { to } : {}) },
      ...(secret ? { tokenSecretRef: { name: secret, ...(key ? { key } : {}) } } : {}),
      ...(Object.keys(constraints).length > 0 ? { constraints } : {}),
    },
  };
}

/** The stored manifest back as the form. */
export function fromDataAgreementManifest(manifest: unknown): DataAgreementForm {
  const envelope = (manifest ?? {}) as { metadata?: { name?: string }; spec?: Record<string, unknown> };
  const spec = envelope.spec ?? {};
  const validity = (spec.validity ?? {}) as { from?: string; to?: string };
  const secret = spec.tokenSecretRef as { name?: string; key?: string } | undefined;
  const constraints = (spec.constraints ?? {}) as DataAgreementForm["constraints"];
  const offer = nameOf(spec.offerRef);
  return {
    name: envelope.metadata?.name ?? "",
    ...(typeof spec.role === "string" ? { role: spec.role } : {}),
    ...(offer ? { offerRef: offer } : {}),
    ...(typeof spec.remoteParticipant === "string" ? { remoteParticipant: spec.remoteParticipant } : {}),
    ...(typeof spec.agreementId === "string" ? { agreementId: spec.agreementId } : {}),
    ...(typeof spec.state === "string" ? { state: spec.state } : {}),
    validity: {
      ...(validity.from ? { from: validity.from } : {}),
      ...(validity.to ? { to: validity.to } : {}),
    },
    ...(secret?.name ? { tokenSecretRef: { name: secret.name, ...(secret.key ? { key: secret.key } : {}) } } : {}),
    ...(constraints && Object.keys(constraints).length > 0 ? { constraints } : {}),
  };
}
