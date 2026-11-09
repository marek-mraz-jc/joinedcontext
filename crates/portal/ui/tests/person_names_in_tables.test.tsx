// covers (T-2137, the module gate in gate_modules.test.ts): src/components/PersonName.tsx.
/**
 * UI judge on the prod rehearsal (jc-ui-tables, 2026-11-09): pages printed bare Keycloak ids
 * where a person belongs (Policies' "Granted to" and their sentences, Members, Groups, a service
 * account's owner). A person reads by name, or as "Unknown person" with the id on hover; an
 * e-mail is words already and stays.
 */
import { render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { personLabel, withPersonNames } from "../src/api/personNames";
import { granteeName } from "../src/components/AccessWords";
import { PersonName } from "../src/components/PersonName";
import { policySentence } from "../src/pages/policies/policySentence";
import { expectStacked } from "./stacked";

const OWNER = "fc3b55df-5fd4-4679-af5e-bb9dca22b0d7";
const t = i18n.t.bind(i18n) as (key: string, options?: Record<string, unknown>) => string;

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("a person in a page's text", () => {
  it("reads by name, as an unknown person, or as the words it already is", () => {
    const names = new Map([[OWNER, "Jana Kováčová"]]);
    expect(personLabel(OWNER, names)).toBe("Jana Kováčová");
    expect(personLabel(OWNER, new Map())).toBe(en.personName.unknown);
    expect(personLabel("jana@hel.fi", new Map())).toBe("jana@hel.fi");
    expect(withPersonNames(`${OWNER} belongs in the group`, new Map())).toBe(`${en.personName.unknown} belongs in the group`);
  });

  it("keeps the id on hover only", () => {
    render(<PersonName id={OWNER} names={new Map()} />);
    expect(screen.getByText(en.personName.unknown)).toHaveAttribute("title", OWNER);
  });

  it("names a policy's person grantee in words, in its sentence too", () => {
    expect(granteeName("user", OWNER, t)).toBe(en.personName.unknown);
    expect(granteeName("user", OWNER, t, new Map([[OWNER, "Jana Kováčová"]]))).toBe("Jana Kováčová");
    const sentence = policySentence(
      { assignee: { kind: "user", id: OWNER }, operations: ["queryEntity"], contextSpaceRef: "air" },
      t,
      "en",
    );
    expect(sentence).not.toContain(OWNER);
    expect(sentence).toContain(en.personName.unknown);
  });
});

describe("the Policies list", () => {
  it("shows no bare id for a person and stacks its rows on a phone", async () => {
    window.history.pushState({}, "", "/projects/helsinki/policies");
    const policy = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Policy",
      metadata: { name: "owner-reads", namespace: "helsinki" },
      spec: { contextSpaceRef: "air", assigner: "did:web:hel.fi", assignee: { kind: "user", id: OWNER }, operations: ["retrieveOps"] },
      status: { phase: "Live" },
    };
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL((input as Request).url);
        const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } });
        if (url.pathname.endsWith("/auth/me")) return json({ subject: "s", username: "demo.steward", roles: ["portal-admin"] });
        if (url.pathname === "/api/v1/projects") return json({ items: [{ name: "helsinki" }] });
        if (url.pathname === "/api/v1/projects/helsinki/policies") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [policy] });
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
      }),
    );
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const row = (await screen.findByRole("link", { name: "owner-reads" })).closest("tr") as HTMLElement;
    expect(row.textContent).not.toContain(OWNER);
    expect(within(row).getAllByText(new RegExp(en.personName.unknown)).length).toBeGreaterThan(0);
    expectStacked(row.closest("table") as HTMLElement, [
      en.policies.field.space,
      en.policies.field.assignee,
      en.policies.field.grants,
      en.resourceList.phase,
    ]);
  });
});
