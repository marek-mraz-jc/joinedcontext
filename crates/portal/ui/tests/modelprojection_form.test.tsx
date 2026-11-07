/**
 * T-1548 (MP-01, MP-02, MF-06, UI-01, UI-44): a ModelProjection is created and edited through a form on
 * its project list, besides the endpoint form that writes one as a side effect.
 *
 * The form writes what `ModelProjectionSpec` declares: the model as a typed reference at a served
 * major, each class with its kept slots (none is identity only, MP-01), a filter only when one is
 * given. A new projection carries its space as the label its repository path is derived from; an
 * edit keeps the stored labels, so the file never moves.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import validator from "../src/components/forms/validator";
import { ResourceListPage } from "../src/routes/ResourceListPage";
import {
  fromModelProjectionManifest,
  modelProjectionSchema,
  toModelProjectionManifest,
} from "../src/schemas/modelprojection";
import type { ModelProjectionForm } from "../src/schemas/modelprojection";
import en from "../src/locales/en.json";
import { digestOf } from "../src/api/digest";
import { json, list, renderPage } from "./page_contract";

const PROJECT = "banskabystrica";
const t = (key: string): string => i18n.t(key);

const FILLED: ModelProjectionForm = {
  name: "air-public",
  contextSpaceRef: "air",
  dataModelRef: { name: "air", version: "1" },
  classes: [{ name: "AirQualityObserved", slots: ["pm10"] }, { name: "Device", slots: [] }],
  filter: { q: "pm10<50" },
};

describe("the ModelProjection manifest the form writes", () => {
  // MP-01, MF-06: a new projection is filed under its space; identity-only classes keep `slots: []`.
  it("is what jc-core reads, labelled with its space, and reads back as the same form", () => {
    const manifest = toModelProjectionManifest(PROJECT, FILLED) as { kind: string; metadata: unknown; spec: unknown };
    expect(manifest.kind).toBe("ModelProjection");
    expect(manifest.metadata).toEqual({
      name: "air-public",
      namespace: PROJECT,
      labels: { "joinedcontext.com/space": "air" },
    });
    expect(manifest.spec).toEqual({
      contextSpaceRef: "air",
      dataModelRef: { kind: "DataModel", name: "air", version: "1" },
      classes: [
        { name: "AirQualityObserved", slots: ["pm10"] },
        { name: "Device", slots: [] },
      ],
      filter: { q: "pm10<50" },
    });
    expect(fromModelProjectionManifest(manifest)).toEqual(FILLED);
  });

  // MF-06: an edit keeps the stored labels, empty rows and an empty filter are dropped.
  it("keeps the stored labels on an edit and drops empty rows and an empty filter", () => {
    const manifest = toModelProjectionManifest(
      PROJECT,
      { ...FILLED, contextSpaceRef: "air", classes: [{ name: "Device", slots: [" ", "status"] }, { name: " " }], filter: { q: " " } },
      { metadata: { name: "air-public", labels: { team: "data" } } },
    ) as { metadata: unknown; spec: Record<string, unknown> };
    expect(manifest.metadata).toEqual({ name: "air-public", namespace: PROJECT, labels: { team: "data" } });
    expect(manifest.spec.classes).toEqual([{ name: "Device", slots: ["status"] }]);
    expect(manifest.spec).not.toHaveProperty("filter");
  });

  // MP-01, DM-22: no class, a version that is not a major, a slot with a dot are refused at the field.
  it("refuses at the field no class, a version that is not a served major, and a slot that is not a slot name", () => {
    const refused = (data: unknown) =>
      validator
        .validateFormData(data, modelProjectionSchema(t))
        .errors.map((error) => (error.property ?? "").replace(/^\./, ""));
    expect(refused(FILLED)).toEqual([]);
    expect(refused({ ...FILLED, classes: [] })).toContain("classes");
    expect(refused({ ...FILLED, dataModelRef: { name: "air", version: "1.3.0" } })).toContain("dataModelRef.version");
    expect(refused({ ...FILLED, classes: [{ name: "Device", slots: ["a.b"] }] })).toContain("classes.0.slots.0");
  });
});

function page(verbs: string[]) {
  const sent: { dryRun: boolean; body: Record<string, unknown> }[] = [];
  renderPage(<ResourceListPage project={PROJECT} plural="projections" />, {
    path: `/projects/${PROJECT}/projections`,
    answer: async (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) {
        return json({ project: PROJECT, bootstrap: false, grants: [{ rule: { kinds: ["ModelProjection"], verbs } }] });
      }
      if (url.pathname.endsWith("/projections") && request.method === "POST") {
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
                metadata: { name: "chg-00000101", namespace: PROJECT },
                status: { lane: "yellow", phase: "PendingApproval" },
              },
              202,
            );
      }
      if (url.pathname.endsWith("/projections")) return json(list([]));
      return undefined;
    },
  });
  return { sent };
}

describe("the Model projections page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const newLabel = en.kinds.ModelProjection.new;

  // UI-01, PF-57: New opens the form; checked, then proposed.
  it("creates a projection through the form, checked before it is proposed", async () => {
    const user = userEvent.setup();
    const { sent } = page(["read", "propose"]);
    await user.click((await screen.findAllByRole("button", { name: newLabel }))[0]);
    const dialog = await screen.findByRole("dialog");
    const field = (id: string) => dialog.querySelector<HTMLElement>(`#root_${id}`)!;
    await user.type(field("name"), "air-public");
    await user.type(field("contextSpaceRef"), "air");
    await user.type(field("dataModelRef_name"), "air");
    await user.type(field("classes_0_name"), "Device");
    const propose = within(dialog).getByRole("button", { name: en.resourceList.propose });
    if (propose.getAttribute("aria-disabled") === "true") {
      await user.click(within(dialog).getByRole("button", { name: en.form.check }));
      await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled", "true"));
    }
    await user.click(propose);
    await waitFor(() => expect(sent.filter((one) => !one.dryRun)).toHaveLength(1));
    const proposed = sent.find((one) => !one.dryRun)!.body as { metadata: unknown; spec: unknown };
    expect(proposed.metadata).toEqual({ name: "air-public", namespace: PROJECT, labels: { "joinedcontext.com/space": "air" } });
    expect(proposed.spec).toEqual({
      contextSpaceRef: "air",
      dataModelRef: { kind: "DataModel", name: "air", version: "1" },
      classes: [{ name: "Device", slots: [] }],
    });
  });

  // UI-44, PF-50: a person who may not propose one finds New disabled, with the reason.
  it("keeps New disabled, with the reason, for a person who may not propose one", async () => {
    page(["read"]);
    const buttons = await screen.findAllByRole("button", { name: newLabel });
    await waitFor(() => expect(buttons[0]).toHaveAttribute("aria-disabled", "true"));
    expect(buttons[0]).toHaveAccessibleDescription(/propose/);
  });
});
