/**
 * T-1543 (DS-07, DS-08, DS-14, UI-01, UI-44): a DataOffer is created and edited through a form on its
 * project list, like every other kind a person authors.
 *
 * The form writes the manifest jc-core reads: endpoints as typed references, the ODRL offer as the
 * object it parses to (text that does not parse travels as written, for jc-core to refuse by field),
 * a purpose only when one is given, and nothing `DataOfferSpec` does not declare.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import validator from "../src/components/forms/validator";
import { ResourceListPage } from "../src/routes/ResourceListPage";
import { OPEN_OFFER, dataOfferSchema, fromDataOfferManifest, toDataOfferManifest } from "../src/schemas/dataoffer";
import type { DataOfferForm } from "../src/schemas/dataoffer";
import en from "../src/locales/en.json";
import { digestOf } from "../src/api/digest";
import { json, list, renderPage } from "./page_contract";

const PROJECT = "banskabystrica";
const t = (key: string): string => i18n.t(key);

/** The members `DataOfferSpec` declares; jc-core refuses any other (`deny_unknown_fields`). */
const SPEC_MEMBERS = ["contextSpaceRef", "endpointRefs", "policy", "containsPersonalData", "purpose"];

const FILLED: DataOfferForm = {
  name: "air-offer",
  contextSpaceRef: "air",
  endpointRefs: ["air-public"],
  policy: OPEN_OFFER,
  containsPersonalData: false,
};

describe("the DataOffer manifest the form writes", () => {
  // DS-07: typed endpoint references and the offer as an object.
  it("is what jc-core reads: typed endpoints, the offer as an object, nothing it does not declare", () => {
    const manifest = toDataOfferManifest(PROJECT, FILLED) as { kind: string; metadata: unknown; spec: Record<string, unknown> };
    expect(manifest.kind).toBe("DataOffer");
    expect(manifest.metadata).toEqual({
      name: "air-offer",
      namespace: PROJECT,
      labels: { "joinedcontext.com/space": "air" },
    });
    expect(manifest.spec).toEqual({
      contextSpaceRef: "air",
      endpointRefs: [{ kind: "Endpoint", name: "air-public" }],
      policy: { "@type": "Offer", permission: [{ action: "use" }] },
      containsPersonalData: false,
    });
    expect(Object.keys(manifest.spec).filter((member) => !SPEC_MEMBERS.includes(member))).toEqual([]);
    expect(fromDataOfferManifest(manifest)).toEqual(FILLED);
  });

  // DS-07, DS-14: empty rows dropped, a bare reference read back, an unparsed offer kept for jc-core.
  it("drops empty endpoint rows, reads bare references, and keeps an offer that is not JSON as written", () => {
    const manifest = toDataOfferManifest(
      PROJECT,
      { ...FILLED, endpointRefs: [" air-public ", ""], policy: "permission: use", containsPersonalData: true, purpose: " " },
      { metadata: { name: "air-offer", title: { en: "Air offer" } } },
    ) as { metadata: unknown; spec: Record<string, unknown> };
    // An edit keeps the stored labels: a new space label would move the file (MF-06).
    expect(manifest.metadata).toEqual({ name: "air-offer", namespace: PROJECT, title: { en: "Air offer" } });
    expect(manifest.spec).toEqual({
      contextSpaceRef: "air",
      endpointRefs: [{ kind: "Endpoint", name: "air-public" }],
      policy: "permission: use",
      containsPersonalData: true,
    });
    expect(fromDataOfferManifest({ spec: { endpointRefs: ["air-public"] } }).endpointRefs).toEqual(["air-public"]);
  });

  // DS-07, DS-14: refused at the field before anything is sent.
  it("refuses at the field no endpoint, a repeated endpoint, and a purpose outside DPV", () => {
    const refused = (data: unknown) =>
      validator
        .validateFormData(data, dataOfferSchema(t))
        .errors.map((error) => (error.property ?? "").replace(/^\./, ""));
    expect(refused(FILLED)).toEqual([]);
    expect(refused({ ...FILLED, endpointRefs: [] })).toContain("endpointRefs");
    expect(refused({ ...FILLED, endpointRefs: ["a", "a"] })).toContain("endpointRefs");
    expect(refused({ ...FILLED, purpose: "https://example.org/purpose" })).toContain("purpose");
  });
});

function page(verbs: string[]) {
  const sent: { dryRun: boolean; body: Record<string, unknown> }[] = [];
  renderPage(<ResourceListPage project={PROJECT} plural="dataoffers" />, {
    path: `/projects/${PROJECT}/dataoffers`,
    answer: async (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) {
        return json({ project: PROJECT, bootstrap: false, grants: [{ rule: { kinds: ["DataOffer"], verbs } }] });
      }
      if (url.pathname.endsWith("/dataoffers") && request.method === "POST") {
        const dryRun = url.searchParams.get("dryRun") === "All";
        const body = (await request.json()) as Record<string, unknown>;
        sent.push({ dryRun, body });
        const judged = Object.fromEntries(Object.entries(body).filter(([key]) => key !== "draft"));
        return dryRun
          ? json({ valid: true, verdict: { ok: true, findings: [], checkedAt: new Date().toISOString(), inputDigest: digestOf(judged) } })
          : json(
              {
                apiVersion: "joinedcontext.com/v1alpha1",
                kind: "Change",
                metadata: { name: "chg-00000071", namespace: PROJECT },
                status: { lane: "yellow", phase: "PendingApproval" },
              },
              202,
            );
      }
      if (url.pathname.endsWith("/dataoffers")) return json(list([]));
      return undefined;
    },
  });
  return { sent };
}

describe("the DataOffers page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const newLabel = en.kinds.DataOffer.new;

  // UI-01, PF-57: New opens the form with the open offer filled; checked, then proposed.
  it("creates an offer through the form, checked before it is proposed", async () => {
    const user = userEvent.setup();
    const { sent } = page(["read", "propose"]);
    await user.click((await screen.findAllByRole("button", { name: newLabel }))[0]);
    const dialog = await screen.findByRole("dialog");
    const field = (id: string) => dialog.querySelector<HTMLElement>(`#root_${id}`)!;
    await user.type(field("name"), "air-offer");
    await user.type(field("contextSpaceRef"), "air");
    await user.type(field("endpointRefs_0"), "air-public");
    const propose = within(dialog).getByRole("button", { name: en.resourceList.propose });
    if (propose.getAttribute("aria-disabled") === "true") {
      await user.click(within(dialog).getByRole("button", { name: en.form.check }));
      await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled", "true"));
    }
    await user.click(propose);

    await waitFor(() => expect(sent.filter((one) => !one.dryRun)).toHaveLength(1));
    const proposed = sent.find((one) => !one.dryRun)!.body as { metadata: unknown; spec: unknown };
    expect(proposed.metadata).toEqual({ name: "air-offer", namespace: PROJECT, labels: { "joinedcontext.com/space": "air" } });
    expect(proposed.spec).toEqual((toDataOfferManifest(PROJECT, FILLED) as { spec: unknown }).spec);
  });

  // UI-44, PF-50: a person who may not propose one finds New disabled, with the reason.
  it("keeps New disabled, with the reason, for a person who may not propose one", async () => {
    page(["read"]);
    const buttons = await screen.findAllByRole("button", { name: newLabel });
    await waitFor(() => expect(buttons[0]).toHaveAttribute("aria-disabled", "true"));
    expect(buttons[0]).toHaveAccessibleDescription(/propose/);
  });
});
