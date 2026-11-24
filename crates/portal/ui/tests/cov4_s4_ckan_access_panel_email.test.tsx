// T-7706: CkanAccessPanel refuses a person whose id is not a Keycloak user id (ADR-N-061) and sends nothing.
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { CkanAccessPanel } from "../src/pages/ckan/CkanAccessPanel";
import { list, renderPage } from "./page_contract";

const V = "joinedcontext.com/v1alpha1";
const ORG_ADMIN = {
  apiVersion: V, kind: "Role", metadata: { name: "org-admin", namespace: "org" },
  spec: { rules: [{ kinds: ["CkanInstance", "RoleBinding", "Role"], verbs: ["propose", "approve", "delete"] }] },
};

describe("CkanAccessPanel, a person typed as an e-mail (T-7706)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("says to pick a person from the list when the id is no user id, and proposes nothing", async () => {
    const writes: string[] = [];
    const reply = (status: number, body: unknown) =>
      new Response(JSON.stringify(body), {
        status,
        headers: { "Content-Type": status < 300 ? "application/json" : "application/problem+json" },
      });
    renderPage(<CkanAccessPanel project="helsinki" instance="hel-fi" known={["hel-fi"]} />, {
      path: "/projects/helsinki/ckan",
      answer: async (url, request) => {
        const path = url.pathname;
        if (path.endsWith("/permissions/me")) {
          return reply(200, { project: "org", bootstrap: false, grants: [{ rule: { kinds: ["Role", "RoleBinding", "Group", "Person"], verbs: ["read", "propose", "approve"] } }] });
        }
        if (request.method !== "GET") {
          writes.push(`${request.method} ${path}`);
          return reply(202, {});
        }
        if (path === "/api/v1/projects/org/roles") return reply(200, list([ORG_ADMIN]));
        if (path === "/api/v1/projects/org/rolebindings") return reply(200, list([]));
        if (path === "/api/v1/projects/org/groups") return reply(200, list([]));
        if (path === "/api/v1/organization/people") {
          return reply(200, { items: [{ id: "jana@hel.fi", email: "jana@hel.fi", firstName: "Jana", lastName: "K", enabled: true, emailVerified: true, requiredActions: [] }], total: 1 });
        }
        if (path === "/api/v1/projects/helsinki/endpoints") return reply(200, list([]));
        return undefined;
      },
    });
    const section = (await screen.findByRole("heading", { name: "Access to hel-fi" })).closest("section") as HTMLElement;
    const add = await within(section).findByRole("button", { name: en.ckan.access.addShort });
    await waitFor(() => expect(section).not.toHaveTextContent(en.app.loading));
    const user = userEvent.setup();
    await user.selectOptions(within(section).getByLabelText(en.ckan.access.kind), "user");
    await user.click(within(section).getByRole("combobox", { name: en.ckan.access.email }));
    await user.click(await screen.findByRole("option", { name: /Jana K/ }));
    await user.click(add);
    expect(await within(section).findByText(en.ckan.access.refused.badEmail)).toBeInTheDocument();
    expect(writes).toEqual([]);
  });
});
