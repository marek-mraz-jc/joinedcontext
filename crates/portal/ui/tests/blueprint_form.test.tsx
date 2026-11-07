/**
 * T-1537 (CC-23…CC-27, CC-59, UI-01, UI-44): a Blueprint is created and edited on Organization →
 * Blueprints through a form, like every other kind a person authors.
 *
 * The parameter schema is written as JSON and stored as the object it parses to; text that does not
 * parse travels as written, so jc-core refuses it naming `spec.parameterSchema` rather than the form
 * dropping it. A blueprint nobody may run, or one with no template, is refused at the field.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import validator from "../src/components/forms/validator";
import { KindList } from "../src/routes/ResourceListPage";
import { blueprintSchema, fromBlueprintManifest, toBlueprintManifest } from "../src/schemas/blueprint";
import type { BlueprintForm } from "../src/schemas/blueprint";
import en from "../src/locales/en.json";
import { digestOf } from "../src/api/digest";
import { json, list, renderPage } from "./page_contract";

const t = (key: string): string => i18n.t(key);

const FILLED: BlueprintForm = {
  name: "weather-feed",
  version: "1.2.0",
  category: "ingest",
  riskClass: "yellow",
  allowedRoles: ["steward"],
  parameterSchema: JSON.stringify({ type: "object", properties: { city: { type: "string" } } }, null, 2),
  templates: [{ name: "source", template: "kind: DataSource\nmetadata:\n  name: '{{ city }}'\n" }],
};

describe("the Blueprint manifest the form writes", () => {
  it("stores the parameter schema as the object it parses to, and reads it back as the same JSON", () => {
    const manifest = toBlueprintManifest(FILLED) as { kind: string; metadata: unknown; spec: Record<string, unknown> };
    expect(manifest.kind).toBe("Blueprint");
    expect(manifest.metadata).toEqual({ name: "weather-feed", namespace: "org" });
    expect(manifest.spec).toEqual({
      version: "1.2.0",
      category: "ingest",
      riskClass: "yellow",
      allowedRoles: ["steward"],
      parameterSchema: { type: "object", properties: { city: { type: "string" } } },
      templates: [{ name: "source", template: "kind: DataSource\nmetadata:\n  name: '{{ city }}'\n" }],
    });
    expect(fromBlueprintManifest(manifest)).toEqual(FILLED);
  });

  it("keeps a parameter schema that is not JSON as written, for jc-core to refuse by its field", () => {
    const manifest = toBlueprintManifest({ ...FILLED, category: " ", parameterSchema: "type: object" }) as {
      spec: Record<string, unknown>;
    };
    expect(manifest.spec.parameterSchema).toBe("type: object");
    expect(manifest.spec).not.toHaveProperty("category");
    expect(fromBlueprintManifest(manifest).parameterSchema).toBe("type: object");
  });

  it("refuses at the field a version that is not x.y.z, no role, and no template", () => {
    const refused = (data: unknown) =>
      validator
        .validateFormData(data, blueprintSchema(t))
        .errors.map((error) => (error.property ?? "").replace(/^\./, ""));
    expect(refused(FILLED)).toEqual([]);
    expect(refused({ ...FILLED, version: "1.2" })).toContain("version");
    expect(refused({ ...FILLED, allowedRoles: [] })).toContain("allowedRoles");
    expect(refused({ ...FILLED, templates: [] })).toContain("templates");
  });
});

function tab(verbs: string[]) {
  const sent: { dryRun: boolean; body: Record<string, unknown> }[] = [];
  renderPage(<KindList project="org" plural="blueprints" embedded />, {
    path: "/organization/blueprints",
    answer: async (url, request) => {
      if (url.pathname.endsWith("/org/permissions/me")) {
        return json({ project: "org", bootstrap: false, grants: [{ rule: { kinds: ["Blueprint"], verbs } }] });
      }
      if (url.pathname.endsWith("/org/blueprints") && request.method === "POST") {
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
                metadata: { name: "chg-00000051", namespace: "org" },
                status: { lane: "red", phase: "PendingApproval" },
              },
              202,
            );
      }
      if (url.pathname.endsWith("/org/blueprints")) return json(list([]));
      return undefined;
    },
  });
  return { sent };
}

describe("Organization → Blueprints", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const newLabel = en.kinds.Blueprint.new;

  // UI-01, PF-57: New opens the form under a heading of the tab's level; checked, then proposed.
  it("creates a blueprint through the form, checked before it is proposed", async () => {
    const user = userEvent.setup();
    const { sent } = tab(["read", "propose"]);
    expect(await screen.findByRole("heading", { level: 2, name: en.nav.blueprints })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 1 })).toBeNull();
    await user.click(screen.getAllByRole("button", { name: newLabel })[0]);
    const dialog = await screen.findByRole("dialog");
    // A blueprint needs one role and one template, so the form opens with one of each to fill.
    const field = (id: string) => dialog.querySelector<HTMLElement>(`#root_${id}`)!;
    await user.type(field("name"), "weather-feed");
    await user.type(field("version"), "1.0.0");
    await user.type(field("allowedRoles_0"), "steward");
    await user.click(field("parameterSchema"));
    await user.paste('{"type": "object"}');
    await user.type(field("templates_0_name"), "source");
    await user.click(field("templates_0_template"));
    await user.paste("kind: DataSource");
    const propose = within(dialog).getByRole("button", { name: en.resourceList.propose });
    if (propose.getAttribute("aria-disabled") === "true") {
      await user.click(within(dialog).getByRole("button", { name: en.form.check }));
      await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled", "true"));
    }
    await user.click(propose);

    await waitFor(() => expect(sent.filter((one) => !one.dryRun)).toHaveLength(1));
    const proposed = sent.find((one) => !one.dryRun)!.body as { metadata: unknown; spec: unknown };
    expect(proposed.metadata).toEqual({ name: "weather-feed", namespace: "org" });
    expect(proposed.spec).toEqual({
      version: "1.0.0",
      riskClass: "yellow",
      allowedRoles: ["steward"],
      parameterSchema: { type: "object" },
      templates: [{ name: "source", template: "kind: DataSource" }],
    });
  });

  // UI-44, PF-50: a person who may not propose one finds New disabled, with the reason.
  it("keeps New disabled, with the reason, for a person who may not propose one", async () => {
    tab(["read"]);
    const buttons = await screen.findAllByRole("button", { name: newLabel });
    await waitFor(() => expect(buttons[0]).toHaveAttribute("aria-disabled", "true"));
    expect(buttons[0]).toHaveAccessibleDescription(/Only an organization administrator can add or change/);
  });
});
