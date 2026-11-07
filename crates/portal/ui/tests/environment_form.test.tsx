/**
 * T-1545 (CC-73, CC-74, CC-75, UI-01, UI-44): an Environment is created and edited on Organization →
 * Environments through a form.
 *
 * The manifest's maps (hosts, images, feature flags) are rows in the form and maps again on the way
 * out; an empty block is left out; an image is pinned by digest; the secret backend is named and no
 * field takes a secret (CC-75).
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import validator from "../src/components/forms/validator";
import { KindList } from "../src/routes/ResourceListPage";
import { environmentSchema, fromEnvironmentManifest, toEnvironmentManifest } from "../src/schemas/environment";
import type { EnvironmentForm } from "../src/schemas/environment";
import en from "../src/locales/en.json";
import { digestOf } from "../src/api/digest";
import { json, list, renderPage } from "./page_contract";

const t = (key: string): string => i18n.t(key);
const DIGEST = `sha256:${"a".repeat(64)}`;

const FILLED: EnvironmentForm = {
  name: "staging",
  orgDomain: "staging.example.org",
  hosts: [{ component: "portal", host: "portal.staging.example.org" }],
  images: [{ component: "portal", digest: DIGEST }],
  secrets: { backend: "openbao", mount: "jc/staging" },
  features: [{ name: "publicEndpoints", enabled: false }],
};

describe("the Environment manifest the form writes", () => {
  // CC-73, CC-74: rows become the maps jc-core reads, and read back as the same rows.
  it("writes each block of rows as the map jc-core reads, and reads it back as the same form", () => {
    const manifest = toEnvironmentManifest(FILLED) as { kind: string; metadata: unknown; spec: unknown };
    expect(manifest.kind).toBe("Environment");
    expect(manifest.metadata).toEqual({ name: "staging", namespace: "org" });
    expect(manifest.spec).toEqual({
      orgDomain: "staging.example.org",
      hosts: { portal: "portal.staging.example.org" },
      images: { portal: DIGEST },
      secrets: { backend: "openbao", mount: "jc/staging" },
      features: { publicEndpoints: false },
    });
    expect(fromEnvironmentManifest(manifest)).toEqual(FILLED);
  });

  it("leaves out every empty block and every row without its name or value", () => {
    const manifest = toEnvironmentManifest({
      name: "staging",
      orgDomain: " ",
      hosts: [{ component: "portal", host: "" }, { component: " ", host: "a.example.org" }],
      images: [],
      secrets: { backend: "", mount: " " },
      features: [{ name: "" }],
    }) as { spec: unknown };
    expect(manifest.spec).toEqual({});
  });

  // CC-74, CC-75: a tag, a host with a scheme, a domain of one label are refused at the field.
  it("refuses at the field an image by tag, a host with a scheme, and a domain of one label", () => {
    const refused = (data: unknown) =>
      validator
        .validateFormData(data, environmentSchema(t))
        .errors.map((error) => (error.property ?? "").replace(/^\./, ""));
    expect(refused(FILLED)).toEqual([]);
    expect(refused({ ...FILLED, images: [{ component: "portal", digest: "latest" }] })).toContain("images.0.digest");
    expect(refused({ ...FILLED, hosts: [{ component: "portal", host: "https://portal.example.org" }] })).toContain(
      "hosts.0.host",
    );
    expect(refused({ ...FILLED, orgDomain: "localhost" })).toContain("orgDomain");
  });

  // CC-75: an overlay names a backend; nothing in the form could hold a secret value.
  it("has no field a secret value could be typed into", () => {
    const names = JSON.stringify(environmentSchema(t)).toLowerCase();
    expect(names).not.toMatch(/"(value|token|password|key)"/);
  });
});

function tab(verbs: string[]) {
  const sent: { dryRun: boolean; body: Record<string, unknown> }[] = [];
  renderPage(<KindList project="org" plural="environments" embedded />, {
    path: "/organization/environments",
    answer: async (url, request) => {
      if (url.pathname.endsWith("/org/permissions/me")) {
        return json({ project: "org", bootstrap: false, grants: [{ rule: { kinds: ["Environment"], verbs } }] });
      }
      if (url.pathname.endsWith("/org/environments") && request.method === "POST") {
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
                metadata: { name: "chg-00000091", namespace: "org" },
                status: { lane: "red", phase: "PendingApproval" },
              },
              202,
            );
      }
      if (url.pathname.endsWith("/org/environments")) return json(list([]));
      return undefined;
    },
  });
  return { sent };
}

describe("Organization → Environments", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const newLabel = en.kinds.Environment.new;

  // UI-01, PF-57: New opens the form; checked, then proposed.
  it("creates an environment through the form, checked before it is proposed", async () => {
    const user = userEvent.setup();
    const { sent } = tab(["read", "propose"]);
    await user.click((await screen.findAllByRole("button", { name: newLabel }))[0]);
    const dialog = await screen.findByRole("dialog");
    const field = (id: string) => dialog.querySelector<HTMLElement>(`#root_${id}`)!;
    await user.type(field("name"), "staging");
    await user.type(field("orgDomain"), "staging.example.org");
    const propose = within(dialog).getByRole("button", { name: en.resourceList.propose });
    if (propose.getAttribute("aria-disabled") === "true") {
      await user.click(within(dialog).getByRole("button", { name: en.form.check }));
      await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled", "true"));
    }
    await user.click(propose);
    await waitFor(() => expect(sent.filter((one) => !one.dryRun)).toHaveLength(1));
    const proposed = sent.find((one) => !one.dryRun)!.body as { metadata: unknown; spec: unknown };
    expect(proposed.metadata).toEqual({ name: "staging", namespace: "org" });
    expect(proposed.spec).toEqual({ orgDomain: "staging.example.org" });
  });

  // UI-44, PF-50: a person who may not propose one finds New disabled, with the reason.
  it("keeps New disabled, with the reason, for a person who may not propose one", async () => {
    tab(["read"]);
    const buttons = await screen.findAllByRole("button", { name: newLabel });
    await waitFor(() => expect(buttons[0]).toHaveAttribute("aria-disabled", "true"));
    expect(buttons[0]).toHaveAccessibleDescription(/Only an organization administrator can add or change/);
  });
});
