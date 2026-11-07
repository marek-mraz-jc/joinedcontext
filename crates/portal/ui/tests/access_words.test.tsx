/**
 * T-4990: access tables in plain words. A grant reads by area ("Read entities"), a role by its
 * verbs and a count of kinds, a grantee by name; the identifiers stay one click or hover away.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { RuleInWords, areaOf, granteeName, operationAreas, verbsInWords } from "../src/components/AccessWords";
import { presetOfRules } from "../src/components/forms/widgets/accessPresets";

const t = i18n.t.bind(i18n) as (key: string, options?: Record<string, unknown>) => string;

describe("operation areas", () => {
  it("files every CIM 009 name under the area a person reads", () => {
    expect(areaOf("queryEntity")).toBe("read");
    expect(areaOf("retrieveTemporal")).toBe("history");
    expect(areaOf("queryTemporal")).toBe("history");
    expect(areaOf("createSubscription")).toBe("subscribe");
    expect(areaOf("createEntity")).toBe("create");
    expect(areaOf("updateAttrs")).toBe("change");
    expect(areaOf("mergeEntity")).toBe("change");
    expect(areaOf("deleteEntity")).toBe("delete");
    expect(areaOf("purgeEntity")).toBe("delete");
    expect(areaOf("retrieveEntityMap")).toBe("federate");
    expect(areaOf("somethingElse")).toBeNull();
  });

  it("expands a group and keeps an unknown name apart instead of hiding it", () => {
    const { areas, other } = operationAreas(["retrieveOps", "deleteEntity", "bogus"]);
    expect(areas).toEqual([
      { area: "read", operations: ["retrieveEntity", "queryEntity"] },
      { area: "delete", operations: ["deleteEntity"] },
    ]);
    expect(other).toEqual(["bogus"]);
  });
});

describe("names and verbs", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("names an endpoint's roles by their endpoint and leaves any other id as it is", () => {
    expect(granteeName("role", "endpoint:helsinki/app-air-quality", t)).toBe(
      "Every caller of endpoint app-air-quality",
    );
    expect(granteeName("role", "endpoint:helsinki/app-air-quality/steward", t)).toBe(
      "App role steward on endpoint app-air-quality",
    );
    expect(granteeName("role", "steward", t)).toBe("steward");
    expect(granteeName("group", "endpoint:helsinki/x", t)).toBe("endpoint:helsinki/x");
  });

  it("joins verbs as a sentence does", () => {
    expect(verbsInWords(["propose", "approve"], t, "en")).toBe("Propose and approve");
    expect(verbsInWords(["read"], t, "en")).toBe("Read");
  });

  it("names up to three kinds and folds a longer list behind its count", async () => {
    const { unmount } = render(
      <I18nextProvider i18n={i18n}>
        <RuleInWords rule={{ verbs: ["propose"], kinds: ["App"] }} />
      </I18nextProvider>,
    );
    expect(screen.getByText("Propose")).toBeInTheDocument();
    expect(screen.getByText(/applications/)).toBeInTheDocument();
    unmount();

    const kinds = ["ContextSpace", "DataModel", "Mapping", "Policy", "Endpoint"];
    render(
      <I18nextProvider i18n={i18n}>
        <RuleInWords rule={{ verbs: ["propose", "approve"], kinds }} />
      </I18nextProvider>,
    );
    const summary = document.querySelector("summary");
    expect(summary?.textContent).toBe("Propose and approve: 5 kinds");
    await userEvent.click(summary as HTMLElement);
    expect(screen.getByText(/Context Spaces, data models/)).toBeVisible();
  });
});

describe("presetOfRules", () => {
  const read = { kinds: ["Entity", "Mapping", "Role"], verbs: ["read"] };
  it("reads a role's shape as the preset it is", () => {
    expect(presetOfRules([read])).toBe("reader");
    expect(presetOfRules([read, { kinds: ["Entity"], verbs: ["propose"] }])).toBe("writer");
    expect(presetOfRules([read, { kinds: ["Entity", "Mapping"], verbs: ["propose", "approve"] }])).toBe("steward");
    expect(presetOfRules([read, { kinds: ["Pipeline", "DataSource"], verbs: ["propose"] }])).toBe("pipelineWriter");
  });

  it("calls a role written by hand no preset", () => {
    expect(presetOfRules([])).toBeNull();
    expect(presetOfRules([{ kinds: ["App"], verbs: ["propose"] }])).toBeNull();
    // A steward that may approve access is more than a steward.
    expect(presetOfRules([read, { kinds: ["Role"], verbs: ["propose", "approve"] }])).toBeNull();
    expect(presetOfRules([read, { kinds: ["Endpoint"], verbs: ["propose"] }])).toBeNull();
    expect(
      presetOfRules([{ ...read, constraints: [{ field: "x" }] } as { kinds: string[]; verbs: string[] }]),
    ).toBeNull();
  });
});

const POLICY = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Policy",
  metadata: { name: "app-air-quality-1", namespace: "helsinki" },
  spec: {
    contextSpaceRef: "helsinki",
    assigner: "did:web:hel.fi",
    assignee: { kind: "role", id: "endpoint:helsinki/app-air-quality" },
    operations: ["queryEntity", "retrieveEntity", "retrieveTemporal"],
    information: [{ entities: [{ type: "AirQualityObserved" }] }],
  },
  status: { phase: "Live" },
};

describe("the Policies table in words", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/helsinki/policies");
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL((input as Request).url);
        const json = (body: unknown) =>
          new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } });
        if (url.pathname.endsWith("/auth/me")) return json({ subject: "s", username: "demo.steward", roles: ["portal-admin"] });
        if (url.pathname === "/api/v1/projects") return json({ items: [{ name: "helsinki" }] });
        if (url.pathname === "/api/v1/projects/helsinki/policies") {
          return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [POLICY] });
        }
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
      }),
    );
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("shows who and what by name, the operation names one click away", async () => {
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const row = (await screen.findByRole("link", { name: "app-air-quality-1" })).closest("tr") as HTMLElement;
    const grantee = within(row).getByText("Every caller of endpoint app-air-quality");
    expect(grantee.parentElement).toHaveAttribute("title", "role:endpoint:helsinki/app-air-quality");
    expect(within(row).getByText("A role")).toBeInTheDocument();
    expect(within(row).queryByText(/^role:endpoint/)).toBeNull();
    // The sentence under the name reads the same grantee.
    expect(within(row).getByText(/^Every caller of endpoint app-air-quality may /)).toBeInTheDocument();

    const grants = within(row).getByText("Read entities, Read history");
    const operations = within(row).getByText("queryEntity, retrieveEntity, retrieveTemporal");
    expect(operations).not.toBeVisible();
    await userEvent.click(grants);
    expect(operations).toBeVisible();
  });
});
