/**
 * T-2884 (DM-77, DM-78): a published model is shared with the organization from its page, and once
 * the organization holds the copy the page offers to import it in place of the model's own classes.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { parse } from "yaml";
import en from "../src/locales/en.json";
import { adoptOrganizationCopy } from "../src/pages/models/ShareModel";
import { jsonResponse, list, renderRoute } from "./pageHarness";
import type { Manifest } from "../src/api/manifest";

const SHARED = `# the city's air
id: https://hel.fi/models/air
name: air
prefixes:
  aq: https://hel.fi/aq/
default_prefix: aq
imports:
  - linkml:types
classes:
  AirQualityObserved:
    class_uri: aq:AirQualityObserved
    slots: [pm10]
slots:
  pm10: { range: float, slot_uri: aq:pm10 }
`;

/** The project's model since the share: one class more, which only the project has. */
const SOURCE = `${SHARED.replace("    slots: [pm10]\n", "    slots: [pm10]\n  Sensor:\n    class_uri: aq:Sensor\n    slots: [serial]\n")}  serial: { range: string, slot_uri: aq:serial }
`;

const model = (lifecycle: string, linkml = SOURCE) =>
  ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataModel",
    metadata: { name: "air", namespace: "helsinki" },
    spec: { contextSpaceRef: "ilma", linkml, version: "1.2.0", lifecycle, classes: ["AirQualityObserved", "Sensor"] },
  }) as Manifest;

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-org-00000009", namespace: "org" },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 6, update: 0, delete: 0 } },
};

const organization = (origin: unknown) => ({
  items: [{ name: "air", level: "organization", project: "org", version: "1.2.0", lifecycle: "published", classes: ["AirQualityObserved"], origin }],
  smartDataModels: [],
});

function world(options: { lifecycle?: string; linkml?: string; origin?: unknown; share?: Response }) {
  const sent: { method: string; path: string; body: string }[] = [];
  const answer = (path: string, request: Request): Response | undefined => {
    if (request.method !== "GET") {
      void request.clone().text().then((body) => sent.push({ method: request.method, path, body }));
    }
    if (path === "/api/v1/projects/helsinki/datamodels" && request.method === "GET") {
      return jsonResponse(list([model(options.lifecycle ?? "published", options.linkml)]));
    }
    if (path === "/api/v1/projects/helsinki/datamodels/air/share") {
      return options.share ?? jsonResponse(CHANGE, 202);
    }
    if (path === "/api/v1/organization/datamodels") {
      return jsonResponse(options.origin === undefined ? { items: [], smartDataModels: [] } : organization(options.origin));
    }
    if (path === "/api/v1/projects/org/datamodels/air/source") {
      return new Response(SHARED, { status: 200, headers: { "Content-Type": "text/yaml" } });
    }
    if (path === "/api/v1/projects/helsinki/datamodels/air/source" && request.method === "PUT") {
      return jsonResponse({ ...CHANGE, metadata: { name: "chg-0000000a", namespace: "helsinki" } }, 202);
    }
    return undefined;
  };
  return { sent, answer };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("using the organization's copy (DM-78)", () => {
  it("imports the copy in place of what it defines, keeping the rest and every IRI", () => {
    const adopted = parse(adoptOrganizationCopy(SOURCE, SHARED, "org.air.v1")) as Record<string, Record<string, unknown>>;
    expect(adopted.imports).toEqual(["linkml:types", "org.air.v1"]);
    expect(Object.keys(adopted.classes)).toEqual(["Sensor"]);
    expect(Object.keys(adopted.slots)).toEqual(["serial"]);
    expect(adopted.id).toBe("https://hel.fi/models/air");
    expect(adopted.prefixes).toEqual({ aq: "https://hel.fi/aq/" });
    // What the import brings is the copy, and the copy's IRIs are the model's own.
    const shared = parse(SHARED) as Record<string, Record<string, Record<string, unknown>>>;
    const original = parse(SOURCE) as Record<string, Record<string, Record<string, unknown>>>;
    expect(shared.classes.AirQualityObserved.class_uri).toBe(original.classes.AirQualityObserved.class_uri);
    expect(shared.slots.pm10.slot_uri).toBe(original.slots.pm10.slot_uri);
  });

  it("drops a section the copy empties and leaves a source importing it already alone", () => {
    const adopted = parse(adoptOrganizationCopy(SHARED, SHARED, "org.air.v1")) as Record<string, unknown>;
    expect(adopted.classes).toBeUndefined();
    expect(adopted.slots).toBeUndefined();
    const twice = adoptOrganizationCopy(adoptOrganizationCopy(SOURCE, SHARED, "org.air.v1"), SHARED, "org.air.v1");
    expect((parse(twice) as { imports: string[] }).imports).toEqual(["linkml:types", "org.air.v1"]);
    expect(adoptOrganizationCopy("a: [", SHARED, "org.air.v1")).toBe("a: [");
  });

  it("is offered on the model the copy came from and proposes the import", async () => {
    const { sent, answer } = world({ origin: { project: "helsinki", space: "ilma", name: "air", version: "1.2.0" } });
    await renderRoute({ path: "/projects/helsinki/models/air", answer });
    const adopt = await screen.findByRole("button", { name: en.models.share.adopt });
    await waitFor(() => expect(adopt).not.toHaveAttribute("aria-disabled"));
    await userEvent.click(adopt);
    await waitFor(() => expect(sent.some((one) => one.method === "PUT")).toBe(true));
    const put = sent.find((one) => one.method === "PUT");
    const proposed = parse(put?.body ?? "") as { imports: string[]; classes: Record<string, unknown> };
    expect(put?.path).toBe("/api/v1/projects/helsinki/datamodels/air/source");
    expect(proposed.imports).toContain("org.air.v1");
    expect(Object.keys(proposed.classes)).toEqual(["Sensor"]);
  });

  it("is not offered for a copy shared from elsewhere, nor once the model imports it", async () => {
    await renderRoute({
      path: "/projects/helsinki/models/air",
      answer: world({ origin: { project: "espoo", name: "air", version: "1.2.0" } }).answer,
    });
    expect(await screen.findByRole("button", { name: en.models.share.action })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.models.share.adopt })).toBeNull();
    cleanup();
    vi.unstubAllGlobals();

    const importing = SOURCE.replace("  - linkml:types\n", "  - linkml:types\n  - org.air.v1\n");
    await renderRoute({
      path: "/projects/helsinki/models/air",
      answer: world({ linkml: importing, origin: { project: "helsinki", name: "air", version: "1.2.0" } }).answer,
    });
    expect(await screen.findByRole("button", { name: en.models.share.action })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.models.share.adopt })).toBeNull();
  });
});

describe("sharing a model with the organization (DM-77)", () => {
  it("asks first, proposes the share and links the Change that waits for the organization", async () => {
    const { sent, answer } = world({});
    await renderRoute({ path: "/projects/helsinki/models/air", answer });
    await userEvent.click(await screen.findByRole("button", { name: en.models.share.action }));
    const dialog = await screen.findByRole("alertdialog", { name: "Share air with the organization?" });
    expect(sent).toEqual([]);
    await userEvent.click(within(dialog).getByRole("button", { name: en.models.share.confirm }));
    expect(await screen.findByText("chg-org-00000009")).toBeInTheDocument();
    const link = screen.getByRole("link", { name: en.changes.review });
    expect(link).toHaveAttribute("href", "/projects/org/approvals/chg-org-00000009");
    expect(sent.map((one) => `${one.method} ${one.path}`)).toEqual(["POST /api/v1/projects/helsinki/datamodels/air/share"]);
  });

  it("says why when the organization refuses the name", async () => {
    const refusal = jsonResponse(
      { title: "Conflict", status: 409, detail: "the organization already has a data model 'air', shared from project 'espoo' model 'air'" },
      409,
    );
    await renderRoute({ path: "/projects/helsinki/models/air", answer: world({ share: refusal }).answer });
    await userEvent.click(await screen.findByRole("button", { name: en.models.share.action }));
    const dialog = await screen.findByRole("alertdialog");
    await userEvent.click(within(dialog).getByRole("button", { name: en.models.share.confirm }));
    expect(await screen.findByRole("alert")).toHaveTextContent("shared from project 'espoo'");
  });

  it("gives the reason instead of the action for a draft and for a person who may not propose", async () => {
    await renderRoute({ path: "/projects/helsinki/models/air", answer: world({ lifecycle: "draft" }).answer });
    const draft = await screen.findByRole("button", { name: en.models.share.action });
    expect(draft).toHaveAttribute("aria-disabled", "true");
    expect(draft).toHaveAccessibleDescription(en.models.share.notPublished);
    cleanup();
    vi.unstubAllGlobals();

    await renderRoute({
      path: "/projects/helsinki/models/air",
      answer: world({}).answer,
      permissions: { project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["*"], verbs: ["read"] } }] },
    });
    expect(await screen.findByRole("button", { name: en.models.share.action })).toHaveAttribute("aria-disabled", "true");
  });
});
