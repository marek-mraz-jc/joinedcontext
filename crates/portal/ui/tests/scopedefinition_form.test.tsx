/**
 * T-2955 (R19, UI-44, PF-50): a ScopeDefinition is created and edited through a form on its project
 * list, at `/projects/{project}/scopedefinitions`, like every other kind a person authors.
 *
 * The person writes the path; the form derives `isRoot` and `isChildOf` from it, so the manifest
 * always carries the three members jc-core requires to agree, and nothing `ScopeDefinitionSpec`
 * does not declare.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import validator from "../src/components/forms/validator";
import { ResourceListPage } from "../src/routes/ResourceListPage";
import {
  fromScopeDefinitionManifest,
  scopeDefinitionSchema,
  toScopeDefinitionManifest,
} from "../src/schemas/scopedefinition";
import en from "../src/locales/en.json";
import { digestOf } from "../src/api/digest";
import { json, list, renderPage } from "./page_contract";

const PROJECT = "banskabystrica";
const t = (key: string): string => i18n.t(key);

/** The members `ScopeDefinitionSpec` declares; jc-core refuses any other (`deny_unknown_fields`). */
const SPEC_MEMBERS = ["scopeString", "isRoot", "isChildOf"];

describe("the ScopeDefinition manifest the form writes", () => {
  // R19: a node below a root hangs under the path without its last part.
  it("derives isChildOf from the path of a node below a root, and reads back as the form", () => {
    const manifest = toScopeDefinitionManifest(PROJECT, { name: "geo-sk-bb-radvan", scopeString: " /geo/SK/BB/Radvan " }) as {
      kind: string;
      metadata: unknown;
      spec: Record<string, unknown>;
    };
    expect(manifest.kind).toBe("ScopeDefinition");
    expect(manifest.metadata).toEqual({ name: "geo-sk-bb-radvan", namespace: PROJECT });
    expect(manifest.spec).toEqual({ scopeString: "/geo/SK/BB/Radvan", isRoot: false, isChildOf: "/geo/SK/BB" });
    expect(Object.keys(manifest.spec).filter((member) => !SPEC_MEMBERS.includes(member))).toEqual([]);
    expect(fromScopeDefinitionManifest(manifest)).toEqual({ name: "geo-sk-bb-radvan", scopeString: "/geo/SK/BB/Radvan" });
  });

  // R19, ADR 005: a taxonomy root is a root and names no parent.
  it("writes a root with isRoot and no isChildOf, and keeps the stored title on an edit", () => {
    const manifest = toScopeDefinitionManifest(
      PROJECT,
      { name: "geo", scopeString: "/geo" },
      { metadata: { name: "geo", title: { en: "Places" } } },
    ) as { metadata: unknown; spec: Record<string, unknown> };
    expect(manifest.metadata).toEqual({ name: "geo", namespace: PROJECT, title: { en: "Places" } });
    expect(manifest.spec).toEqual({ scopeString: "/geo", isRoot: true });
  });

  // R19: what jc-core refuses is refused at the field before anything is sent.
  it("refuses at the field an unknown root, a trailing slash, an empty or odd segment and a bad name", () => {
    const refused = (data: unknown) =>
      validator
        .validateFormData(data, scopeDefinitionSchema(t))
        .errors.map((error) => (error.property ?? "").replace(/^\./, ""));
    expect(refused({ name: "geo-sk", scopeString: "/geo/SK" })).toEqual([]);
    expect(refused({ name: "admin", scopeString: "/admin" })).toEqual([]);
    for (const scopeString of ["/places/SK", "/geo/SK/", "/geo//SK", "geo/SK", "/geo/S K", "/", ""]) {
      expect(refused({ name: "x", scopeString }), scopeString).toContain("scopeString");
    }
    expect(refused({ name: "Geo SK", scopeString: "/geo/SK" })).toContain("name");
    expect(refused({ name: "x" })).toContain("scopeString");
  });
});

function page(verbs: string[]) {
  const sent: { dryRun: boolean; body: Record<string, unknown> }[] = [];
  renderPage(<ResourceListPage project={PROJECT} plural="scopedefinitions" />, {
    path: `/projects/${PROJECT}/scopedefinitions`,
    answer: async (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) {
        return json({ project: PROJECT, bootstrap: false, grants: [{ rule: { kinds: ["ScopeDefinition"], verbs } }] });
      }
      if (url.pathname.endsWith("/scopedefinitions") && request.method === "POST") {
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
                metadata: { name: "chg-00000072", namespace: PROJECT },
                status: { lane: "red", phase: "PendingApproval" },
              },
              202,
            );
      }
      if (url.pathname.endsWith("/scopedefinitions")) return json(list([]));
      return undefined;
    },
  });
  return { sent };
}

describe("the ScopeDefinitions page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const newLabel = en.kinds.ScopeDefinition.new;

  // UI-44, PF-57: New opens the form; the path is typed, checked, then proposed under its own plural.
  it("creates a scope definition through the form, checked before it is proposed", async () => {
    const user = userEvent.setup();
    const { sent } = page(["read", "propose"]);
    await user.click((await screen.findAllByRole("button", { name: newLabel }))[0]);
    const dialog = await screen.findByRole("dialog");
    const field = (id: string) => dialog.querySelector<HTMLElement>(`#root_${id}`)!;
    await user.type(field("name"), "geo-sk-bb");
    await user.type(field("scopeString"), "/geo/SK/BB");
    const propose = within(dialog).getByRole("button", { name: en.resourceList.propose });
    if (propose.getAttribute("aria-disabled") === "true") {
      await user.click(within(dialog).getByRole("button", { name: en.form.check }));
      await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled", "true"));
    }
    await user.click(propose);

    await waitFor(() => expect(sent.filter((one) => !one.dryRun)).toHaveLength(1));
    const proposed = sent.find((one) => !one.dryRun)!.body as { kind: string; metadata: unknown; spec: unknown };
    expect(proposed.kind).toBe("ScopeDefinition");
    expect(proposed.metadata).toEqual({ name: "geo-sk-bb", namespace: PROJECT });
    expect(proposed.spec).toEqual({ scopeString: "/geo/SK/BB", isRoot: false, isChildOf: "/geo/SK" });
  });

  // UI-44, PF-50: a person who may not propose one finds New disabled, with the reason.
  it("keeps New disabled, with the reason, for a person who may not propose one", async () => {
    page(["read"]);
    const buttons = await screen.findAllByRole("button", { name: newLabel });
    await waitFor(() => expect(buttons[0]).toHaveAttribute("aria-disabled", "true"));
    expect(buttons[0]).toHaveAccessibleDescription(/propose/);
  });
});
