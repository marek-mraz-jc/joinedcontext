/**
 * T-1544 (DS-04, DS-06, DS-07, UI-01, UI-44): the organization's DataSpaceParticipant is created once
 * and edited on Organization → Data space through a form.
 *
 * jc-core keeps one participant per organization at one path whatever its name, so the tab offers New
 * only once the list has answered that there is none: a second participant is never proposed over the
 * first (T-2855 is the server's guard).
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import validator from "../src/components/forms/validator";
import { KindList } from "../src/routes/ResourceListPage";
import {
  dataSpaceParticipantSchema,
  fromDataSpaceParticipantManifest,
  toDataSpaceParticipantManifest,
} from "../src/schemas/dataspaceparticipant";
import type { DataSpaceParticipantForm } from "../src/schemas/dataspaceparticipant";
import en from "../src/locales/en.json";
import { digestOf } from "../src/api/digest";
import { json, list, renderPage } from "./page_contract";

const t = (key: string): string => i18n.t(key);

const FILLED: DataSpaceParticipantForm = {
  name: "participant",
  did: "did:web:participant.example.org",
  credentialIssuer: "https://issuer.example.org",
  connectorUrl: "https://connector.example.org",
  engine: "rainbow",
  trustAnchors: ["did:web:authority.example.org"],
};

describe("the DataSpaceParticipant manifest the form writes", () => {
  // DS-07: what `DataSpaceParticipantSpec` declares, in namespace org.
  it("is what jc-core reads, in namespace org, and reads back as the same form", () => {
    const manifest = toDataSpaceParticipantManifest(FILLED) as { kind: string; metadata: unknown; spec: unknown };
    expect(manifest.kind).toBe("DataSpaceParticipant");
    expect(manifest.metadata).toEqual({ name: "participant", namespace: "org" });
    expect(manifest.spec).toEqual({
      did: "did:web:participant.example.org",
      credentialIssuer: "https://issuer.example.org",
      connectorUrl: "https://connector.example.org",
      engine: "rainbow",
      trustAnchors: ["did:web:authority.example.org"],
    });
    expect(fromDataSpaceParticipantManifest(manifest)).toEqual(FILLED);
  });

  it("writes no trust anchors when every row is empty", () => {
    const manifest = toDataSpaceParticipantManifest({ ...FILLED, trustAnchors: [" ", ""] }) as { spec: object };
    expect(manifest.spec).not.toHaveProperty("trustAnchors");
  });

  // DS-04, DS-07: a DID off did:web, an address without HTTPS, a repeated anchor are refused at the field.
  it("refuses at the field a DID that is not did:web, an address that is not HTTPS, a repeated anchor", () => {
    const refused = (data: unknown) =>
      validator
        .validateFormData(data, dataSpaceParticipantSchema(t))
        .errors.map((error) => (error.property ?? "").replace(/^\./, ""));
    expect(refused(FILLED)).toEqual([]);
    expect(refused({ ...FILLED, did: "did:key:z6Mk" })).toContain("did");
    expect(refused({ ...FILLED, credentialIssuer: "http://issuer.example.org" })).toContain("credentialIssuer");
    expect(refused({ ...FILLED, connectorUrl: "connector.example.org" })).toContain("connectorUrl");
    expect(refused({ ...FILLED, trustAnchors: ["did:web:a.example.org", "did:web:a.example.org"] })).toContain(
      "trustAnchors",
    );
  });
});

function tab(verbs: string[], items: unknown[] = []) {
  const sent: { dryRun: boolean; body: Record<string, unknown> }[] = [];
  renderPage(<KindList project="org" plural="dataspaceparticipants" embedded />, {
    path: "/organization/dataspaceparticipants",
    answer: async (url, request) => {
      if (url.pathname.endsWith("/org/permissions/me")) {
        return json({ project: "org", bootstrap: false, grants: [{ rule: { kinds: ["DataSpaceParticipant"], verbs } }] });
      }
      if (url.pathname.endsWith("/org/dataspaceparticipants") && request.method === "POST") {
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
                metadata: { name: "chg-00000081", namespace: "org" },
                status: { lane: "red", phase: "PendingApproval" },
              },
              202,
            );
      }
      if (url.pathname.endsWith("/org/dataspaceparticipants")) return json(list(items));
      return undefined;
    },
  });
  return { sent };
}

describe("Organization → Data space", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const newLabel = en.kinds.DataSpaceParticipant.new;

  // UI-01, PF-57: with none yet, New opens the form; checked, then proposed.
  it("creates the participant through the form while the organization has none", async () => {
    const user = userEvent.setup();
    const { sent } = tab(["read", "propose"]);
    await user.click((await screen.findAllByRole("button", { name: newLabel }))[0]);
    const dialog = await screen.findByRole("dialog");
    const field = (id: string) => dialog.querySelector<HTMLElement>(`#root_${id}`)!;
    await user.type(field("name"), "participant");
    await user.type(field("did"), "did:web:participant.example.org");
    await user.type(field("credentialIssuer"), "https://issuer.example.org");
    await user.type(field("connectorUrl"), "https://connector.example.org");
    const propose = within(dialog).getByRole("button", { name: en.resourceList.propose });
    if (propose.getAttribute("aria-disabled") === "true") {
      await user.click(within(dialog).getByRole("button", { name: en.form.check }));
      await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled", "true"));
    }
    await user.click(propose);
    await waitFor(() => expect(sent.filter((one) => !one.dryRun)).toHaveLength(1));
    expect((sent.find((one) => !one.dryRun)!.body as { spec: unknown }).spec).toEqual({
      did: "did:web:participant.example.org",
      credentialIssuer: "https://issuer.example.org",
      connectorUrl: "https://connector.example.org",
      engine: "rainbow",
    });
  });

  // DS-07: one per organization; with one standing, the tab offers no New, only the row's Edit.
  it("offers no New once the organization has its participant", async () => {
    tab(["read", "propose"], [
      {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "DataSpaceParticipant",
        metadata: { name: "participant", namespace: "org" },
        spec: { did: FILLED.did, credentialIssuer: FILLED.credentialIssuer, connectorUrl: FILLED.connectorUrl },
      },
    ]);
    expect(await screen.findByText("participant")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: newLabel })).toBeNull();
  });

  // UI-44, PF-50: a person who may not propose one finds New disabled, with the reason.
  it("keeps New disabled, with the reason, for a person who may not propose one", async () => {
    tab(["read"]);
    const buttons = await screen.findAllByRole("button", { name: newLabel });
    await waitFor(() => expect(buttons[0]).toHaveAttribute("aria-disabled", "true"));
    expect(buttons[0]).toHaveAccessibleDescription(/Only an organization administrator can add or change/);
  });
});
