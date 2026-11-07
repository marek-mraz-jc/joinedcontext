/**
 * T-1542 (DS-09, DS-10, DS-15, UI-01, UI-44): a DataAgreement is created and edited through a form on
 * its list, like every other kind a person authors.
 *
 * The form writes the manifest jc-core reads: the offer as a typed reference, the validity as an
 * object, the token's secret by name and never a token, and no empty member. What jc-core refuses on
 * a combination (a provider without its offer, a finalized consumer without its token) it refuses
 * naming the field; what the browser can tell alone (a DID, a date) is refused at the field.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import validator from "../src/components/forms/validator";
import { ResourceListPage } from "../src/routes/ResourceListPage";
import {
  dataAgreementSchema,
  fromDataAgreementManifest,
  toDataAgreementManifest,
} from "../src/schemas/dataagreement";
import type { DataAgreementForm } from "../src/schemas/dataagreement";
import en from "../src/locales/en.json";
import { digestOf } from "../src/api/digest";
import { json, list, renderPage } from "./page_contract";

const PROJECT = "banskabystrica";
const t = (key: string): string => i18n.t(key);

/** The members `DataAgreementSpec` declares; jc-core refuses any other (`deny_unknown_fields`). */
const SPEC_MEMBERS = [
  "role",
  "offerRef",
  "remoteParticipant",
  "agreementId",
  "state",
  "validity",
  "tokenSecretRef",
  "constraints",
];

const FILLED: DataAgreementForm = {
  name: "partner-air",
  role: "provider",
  offerRef: "air-offer",
  remoteParticipant: "did:web:partner.example.org",
  agreementId: "urn:uuid:3f1c2d4e",
  state: "finalized",
  validity: { from: "2027-01-01T00:00:00Z", to: "2027-12-31T23:59:59Z" },
  tokenSecretRef: { name: "partner-token", key: "token" },
  constraints: { q: "pm10<50" },
};

describe("the DataAgreement manifest the form writes", () => {
  it("is what jc-core reads: a typed offer, the validity as an object, nothing it does not declare", () => {
    const manifest = toDataAgreementManifest(PROJECT, FILLED) as {
      kind: string;
      metadata: Record<string, unknown>;
      spec: Record<string, unknown>;
    };
    expect(manifest.kind).toBe("DataAgreement");
    expect(manifest.metadata).toEqual({ name: "partner-air", namespace: PROJECT });
    expect(manifest.spec).toEqual({
      role: "provider",
      offerRef: { kind: "DataOffer", name: "air-offer" },
      remoteParticipant: "did:web:partner.example.org",
      agreementId: "urn:uuid:3f1c2d4e",
      state: "finalized",
      validity: { from: "2027-01-01T00:00:00Z", to: "2027-12-31T23:59:59Z" },
      tokenSecretRef: { name: "partner-token", key: "token" },
      constraints: { q: "pm10<50" },
    });
    expect(Object.keys(manifest.spec).filter((member) => !SPEC_MEMBERS.includes(member))).toEqual([]);
    expect(fromDataAgreementManifest(manifest)).toEqual(FILLED);
  });

  it("writes no empty member, and keeps what the form does not show from the stored manifest", () => {
    const manifest = toDataAgreementManifest(
      PROJECT,
      {
        name: "partner-air",
        role: "consumer",
        offerRef: " ",
        remoteParticipant: " did:web:partner.example.org ",
        agreementId: "a-1",
        state: "requested",
        validity: { from: "" },
        tokenSecretRef: { name: "", key: "token" },
        constraints: { q: "", geoQ: " " },
      },
      { metadata: { name: "partner-air", title: { en: "Partner air" }, labels: { team: "data" } } },
    ) as { metadata: Record<string, unknown>; spec: Record<string, unknown> };
    expect(manifest.metadata).toEqual({
      name: "partner-air",
      namespace: PROJECT,
      title: { en: "Partner air" },
      labels: { team: "data" },
    });
    expect(manifest.spec).toEqual({
      role: "consumer",
      remoteParticipant: "did:web:partner.example.org",
      agreementId: "a-1",
      state: "requested",
      validity: {},
    });
  });

  it("refuses at the field what is not a did:web, not a moment with its offset, or an empty identifier", () => {
    const refused = (data: unknown) =>
      validator
        .validateFormData(data, dataAgreementSchema(t))
        .errors.map((error) => (error.property ?? "").replace(/^\./, ""));
    const base = { name: "partner-air", role: "consumer", agreementId: "a-1", state: "requested" };
    expect(refused({ ...base, remoteParticipant: "did:web:partner.example.org" })).toEqual([]);
    expect(refused({ ...base, remoteParticipant: "did:key:z6Mk" })).toContain("remoteParticipant");
    expect(refused({ ...base, remoteParticipant: "did:web:localhost" })).toContain("remoteParticipant");
    expect(
      refused({ ...base, remoteParticipant: "did:web:partner.example.org", validity: { to: "2027-12-31" } }),
    ).toContain("validity.to");
    expect(refused({ ...base, remoteParticipant: "did:web:partner.example.org", agreementId: "  " })).toContain(
      "agreementId",
    );
  });
});

function page(verbs: string[]) {
  const sent: { dryRun: boolean; body: Record<string, unknown> }[] = [];
  renderPage(<ResourceListPage project={PROJECT} plural="dataagreements" />, {
    path: `/projects/${PROJECT}/dataagreements`,
    answer: async (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) {
        return json({ project: PROJECT, bootstrap: false, grants: [{ rule: { kinds: ["DataAgreement"], verbs } }] });
      }
      if (url.pathname.endsWith("/dataagreements") && request.method === "POST") {
        const dryRun = url.searchParams.get("dryRun") === "All";
        const body = (await request.json()) as Record<string, unknown>;
        sent.push({ dryRun, body });
        return dryRun
          ? json({
              valid: true,
              // The server judges the manifest without the draft it names, as the verdict does.
              verdict: {
                ok: true,
                findings: [],
                checkedAt: new Date().toISOString(),
                inputDigest: digestOf(Object.fromEntries(Object.entries(body).filter(([key]) => key !== "draft"))),
              },
            })
          : json(
              {
                apiVersion: "joinedcontext.com/v1alpha1",
                kind: "Change",
                metadata: { name: "chg-00000042", namespace: PROJECT },
                status: { lane: "yellow", phase: "PendingApproval" },
              },
              202,
            );
      }
      if (url.pathname.endsWith("/dataagreements")) return json(list([]));
      return undefined;
    },
  });
  return { sent };
}

describe("the DataAgreements page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const newLabel = en.kinds.DataAgreement.new;

  // UI-01, PF-57: New opens the form; it is checked, then proposed as the manifest jc-core reads.
  it("creates an agreement through the form, checked before it is proposed", async () => {
    const user = userEvent.setup();
    const { sent } = page(["read", "propose"]);
    expect(await screen.findByText(en.resourceList.emptyHintCreate.replace("{new}", en.kinds.DataAgreement.new))).toBeInTheDocument();
    await user.click(screen.getAllByRole("button", { name: newLabel })[0]);
    const dialog = await screen.findByRole("dialog");
    await user.type(within(dialog).getByLabelText(new RegExp(`^${en.dataagreements.field.name}`)), "partner-air");
    await user.type(
      within(dialog).getByLabelText(new RegExp(`^${en.dataagreements.field.remoteParticipant}`)),
      "did:web:partner.example.org",
    );
    await user.type(within(dialog).getByLabelText(new RegExp(`^${en.dataagreements.field.agreementId}`)), "a-1");
    const propose = within(dialog).getByRole("button", { name: en.resourceList.propose });
    if (propose.getAttribute("aria-disabled") === "true") {
      await user.click(within(dialog).getByRole("button", { name: en.form.check }));
      await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled", "true"));
    }
    await user.click(propose);

    await waitFor(() => expect(sent.filter((one) => !one.dryRun)).toHaveLength(1));
    const proposed = sent.find((one) => !one.dryRun)!.body as { kind: string; metadata: unknown; spec: unknown };
    expect(proposed.kind).toBe("DataAgreement");
    expect(proposed.metadata).toEqual({ name: "partner-air", namespace: PROJECT });
    expect(proposed.spec).toEqual({
      role: "consumer",
      remoteParticipant: "did:web:partner.example.org",
      agreementId: "a-1",
      state: "requested",
      validity: {},
    });
    expect(sent.some((one) => one.dryRun)).toBe(true);
    expect(await screen.findByRole("link", { name: /review/i })).toBeInTheDocument();
  });

  // UI-44, PF-50: a person who may not propose one finds New disabled, with the reason.
  it("keeps New disabled, with the reason, for a person who may not propose one", async () => {
    page(["read"]);
    const buttons = await screen.findAllByRole("button", { name: newLabel });
    await waitFor(() => expect(buttons[0]).toHaveAttribute("aria-disabled", "true"));
    expect(buttons[0]).toHaveAccessibleDescription(/propose/);
  });

  // T-2488: a kind with no create form offers no create action, and says so in its hint.
  it("offers no New for a kind a person does not create from its list", async () => {
    renderPage(<ResourceListPage project={PROJECT} plural="mappings" />, {
      path: `/projects/${PROJECT}/mappings`,
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) {
          return json({ project: PROJECT, bootstrap: false, grants: [{ rule: { kinds: ["Mapping"], verbs: ["propose"] } }] });
        }
        if (url.pathname.endsWith("/mappings")) return json(list([]));
        return undefined;
      },
    });
    expect(await screen.findByText(en.resourceList.emptyHint)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^New/ })).toBeNull();
  });
});
