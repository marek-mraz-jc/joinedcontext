/**
 * T-1536 (AG-26, AG-41, AG-47…AG-50, AG-70, MF-40, UI-01, UI-44): an AgentProfile is created and
 * edited on Organization → Agent profiles through a form, like every other kind a person authors.
 *
 * The form writes the manifest jc-core reads and nothing it does not declare: no empty block, no
 * `access` when none is granted (read-only operations, AG-70), and no field a model key could be
 * typed into. What the browser can tell alone (a digest, a wall clock, a host) is refused at the field.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import validator from "../src/components/forms/validator";
import { KindList } from "../src/routes/ResourceListPage";
import { agentProfileSchema, fromAgentProfileManifest, toAgentProfileManifest } from "../src/schemas/agentprofile";
import type { AgentProfileForm } from "../src/schemas/agentprofile";
import en from "../src/locales/en.json";
import { digestOf } from "../src/api/digest";
import { json, list, renderPage } from "./page_contract";

const t = (key: string): string => i18n.t(key);
const DIGEST = `sha256:${"a".repeat(64)}`;

/** The members `AgentProfileSpec` declares; jc-core refuses any other (`deny_unknown_fields`). */
const SPEC_MEMBERS = ["role", "runtime", "model", "limits", "egress", "tools", "workspace", "access"];

const STEWARD: AgentProfileForm = {
  name: "data-helper",
  role: "steward",
  runtime: { image: "registry.example.org/agents/runner", digest: DIGEST },
  model: { provider: "openai-compatible", name: "example-model-1", maxTokensPerRun: 200000 },
  limits: {
    stepsPerRun: 120,
    wallClock: "PT20M",
    concurrentRunsPerOrganization: 2,
    requestsPerMinute: 60,
    maxResponseBytes: 2097152,
  },
  egress: {},
  workspace: { cpu: "1", memory: "2Gi", ephemeralStorage: "4Gi" },
};

describe("the AgentProfile manifest the form writes", () => {
  // AG-70: a profile that grants nothing carries no access block, which reads as read-only.
  it("is what jc-core reads, in namespace org, with no access block when nothing is granted", () => {
    const manifest = toAgentProfileManifest(STEWARD) as {
      kind: string;
      metadata: unknown;
      spec: Record<string, unknown>;
    };
    expect(manifest.kind).toBe("AgentProfile");
    expect(manifest.metadata).toEqual({ name: "data-helper", namespace: "org" });
    expect(manifest.spec).not.toHaveProperty("access");
    expect(manifest.spec).not.toHaveProperty("tools");
    expect(manifest.spec.egress).toEqual({});
    expect(Object.keys(manifest.spec).filter((member) => !SPEC_MEMBERS.includes(member))).toEqual([]);
    expect(fromAgentProfileManifest(manifest)).toEqual(STEWARD);
  });

  // AG-50, AG-70, MF-40: empty rows are dropped, a grant keeps its verbs, the stored title stays.
  it("carries the stored model routing, which the form has no field for (AG-72)", () => {
    const routing = { order: ["google-ai-studio/flex", "google-ai-studio"], allowFallbacks: true };
    const stored = { spec: { model: { routing } } };
    const manifest = toAgentProfileManifest(STEWARD, stored) as { spec: { model: Record<string, unknown> } };
    expect(manifest.spec.model.routing).toEqual(routing);
    const fresh = toAgentProfileManifest(STEWARD) as { spec: { model: Record<string, unknown> } };
    expect(fresh.spec.model).not.toHaveProperty("routing");
  });

  it("drops empty rows and keeps what the form does not show from the stored manifest", () => {
    const manifest = toAgentProfileManifest(
      {
        ...STEWARD,
        role: "builder",
        model: { ...STEWARD.model, reasoningEffort: "" },
        egress: { allowedHosts: [" docs.example.org ", ""], maxBytesPerRun: 1024 },
        tools: ["shell", "git"],
        access: {
          operations: ["jc_resource_list", " "],
          kinds: [{ kind: "Endpoint", verbs: ["read", "propose"] }, { kind: " ", verbs: ["read"] }],
          endpoints: [{ name: "air-quality", verbs: ["read"] }],
        },
      },
      { metadata: { name: "data-helper", title: { en: "Data helper" } } },
    ) as { metadata: unknown; spec: Record<string, unknown> };
    expect(manifest.metadata).toEqual({ name: "data-helper", namespace: "org", title: { en: "Data helper" } });
    expect(manifest.spec.model).toEqual({ provider: "openai-compatible", name: "example-model-1", maxTokensPerRun: 200000 });
    expect(manifest.spec.egress).toEqual({ allowedHosts: ["docs.example.org"], maxBytesPerRun: 1024 });
    expect(manifest.spec.tools).toEqual(["shell", "git"]);
    expect(manifest.spec.access).toEqual({
      operations: ["jc_resource_list"],
      kinds: [{ kind: "Endpoint", verbs: ["read", "propose"] }],
      endpoints: [{ name: "air-quality", verbs: ["read"] }],
    });
  });

  // AG-41, AG-49, AG-50: refused at the field before anything is sent.
  it("refuses at the field a tag for a digest, a wall clock that is not PT, a host with a scheme, a bad operation", () => {
    const refused = (data: unknown) =>
      validator
        .validateFormData(data, agentProfileSchema(t))
        .errors.map((error) => (error.property ?? "").replace(/^\./, ""));
    expect(refused(STEWARD)).toEqual([]);
    expect(refused({ ...STEWARD, runtime: { image: "runner", digest: "latest" } })).toContain("runtime.digest");
    expect(refused({ ...STEWARD, limits: { ...STEWARD.limits, wallClock: "20m" } })).toContain("limits.wallClock");
    expect(refused({ ...STEWARD, limits: { ...STEWARD.limits, wallClock: "PT" } })).toContain("limits.wallClock");
    expect(refused({ ...STEWARD, egress: { allowedHosts: ["https://docs.example.org"] } })).toContain(
      "egress.allowedHosts.0",
    );
    expect(refused({ ...STEWARD, access: { operations: ["delete_everything"] } })).toContain("access.operations.0");
    expect(refused({ ...STEWARD, model: { ...STEWARD.model, maxTokensPerRun: 0 } })).toContain("model.maxTokensPerRun");
  });

  // Security: the proxy holds the model's key, so no field of the form could take one.
  it("has no field a key or a token could be typed into", () => {
    const names = JSON.stringify(agentProfileSchema(t)).toLowerCase();
    expect(names).not.toMatch(/"(apikey|api_key|key|token|secret|password)"/);
  });
});

function tab(verbs: string[]) {
  const sent: { dryRun: boolean; body: Record<string, unknown> }[] = [];
  renderPage(<KindList project="org" plural="agentprofiles" embedded />, {
    path: "/organization/agentprofiles",
    answer: async (url, request) => {
      if (url.pathname.endsWith("/org/permissions/me")) {
        return json({ project: "org", bootstrap: false, grants: [{ rule: { kinds: ["AgentProfile"], verbs } }] });
      }
      if (url.pathname.endsWith("/org/agentprofiles") && request.method === "POST") {
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
                metadata: { name: "chg-00000061", namespace: "org" },
                status: { lane: "red", phase: "PendingApproval" },
              },
              202,
            );
      }
      if (url.pathname.endsWith("/org/agentprofiles")) return json(list([]));
      return undefined;
    },
  });
  return { sent };
}

describe("Organization → Agent profiles", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const newLabel = en.kinds.AgentProfile.new;

  // UI-01, PF-57: New opens the form with the reference limits filled; checked, then proposed.
  it("creates a steward profile through the form, checked before it is proposed", async () => {
    const user = userEvent.setup();
    const { sent } = tab(["read", "propose"]);
    expect(await screen.findByRole("heading", { level: 2, name: en.nav.agentprofiles })).toBeInTheDocument();
    await user.click(screen.getAllByRole("button", { name: newLabel })[0]);
    const dialog = await screen.findByRole("dialog");
    const field = (id: string) => dialog.querySelector<HTMLElement>(`#root_${id}`)!;
    await user.type(field("name"), "data-helper");
    await user.type(field("runtime_image"), "registry.example.org/agents/runner");
    await user.click(field("runtime_digest"));
    await user.paste(DIGEST);
    await user.type(field("model_name"), "example-model-1");
    await user.type(field("model_maxTokensPerRun"), "200000");
    const propose = within(dialog).getByRole("button", { name: en.resourceList.propose });
    if (propose.getAttribute("aria-disabled") === "true") {
      await user.click(within(dialog).getByRole("button", { name: en.form.check }));
      await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled", "true"));
    }
    await user.click(propose);

    await waitFor(() => expect(sent.filter((one) => !one.dryRun)).toHaveLength(1));
    const proposed = sent.find((one) => !one.dryRun)!.body as { metadata: unknown; spec: Record<string, unknown> };
    expect(proposed.metadata).toEqual({ name: "data-helper", namespace: "org" });
    expect(proposed.spec).toEqual({
      role: "steward",
      runtime: { image: "registry.example.org/agents/runner", digest: DIGEST },
      model: { provider: "openai-compatible", name: "example-model-1", maxTokensPerRun: 200000 },
      limits: STEWARD.limits,
      egress: {},
      workspace: STEWARD.workspace,
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
