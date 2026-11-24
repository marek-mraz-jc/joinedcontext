// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/access/Roles.tsx in the
// organization's scope (Organization → Roles): the organization's own roles, their edit form, the
// empty list, a failed read and a proposal shown in the form when no route holds it (T-7588).
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { Roles } from "../src/pages/access/Roles";
import { answeringChecks, checksSoFar } from "./checks";
import { list, problem, renderPage } from "./page_contract";

const V = "joinedcontext.com/v1alpha1";
const ORG_ROLE = {
  apiVersion: V,
  kind: "Role",
  metadata: { name: "auditor", namespace: "org", title: { en: "Auditor", sk: "Audítor" } },
  spec: { rules: [{ kinds: ["Change"], verbs: ["read"] }] },
};
const CHANGE = {
  apiVersion: V,
  kind: "Change",
  metadata: { name: "chg-0000003c", namespace: "org" },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 1, update: 0, delete: 0 } },
};

function renderRoles({ roles = [ORG_ROLE] as unknown[], failFirst = false } = {}) {
  let reads = 0;
  const posted: unknown[] = [];
  renderPage(<Roles project="org" scope="organization" />, {
    path: "/organization/roles",
    answer: async (url, request) => {
      const path = url.pathname;
      if (path.endsWith("/permissions/me")) {
        return new Response(
          JSON.stringify({ project: "org", bootstrap: false, grants: [{ rule: { kinds: ["Role", "Pipeline"], verbs: ["read", "propose", "approve"] } }] }),
          { headers: { "Content-Type": "application/json" } },
        );
      }
      if (path === "/api/v1/projects/org/roles" && request.method === "POST") {
        posted.push(await request.clone().json());
        return new Response(JSON.stringify(CHANGE), { status: 202, headers: { "Content-Type": "application/json" } });
      }
      if (path === "/api/v1/projects/org/roles/auditor") {
        return new Response(JSON.stringify(ORG_ROLE), { headers: { "Content-Type": "application/json" } });
      }
      if (path === "/api/v1/projects/org/roles") {
        reads += 1;
        if (failFirst && reads === 1) return problem(503, "the role store is restarting");
        return new Response(JSON.stringify(list(roles)), { headers: { "Content-Type": "application/json" } });
      }
      return undefined;
    },
  });
  vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));
  return { posted, reads: () => reads };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the organization's roles", () => {
  it("lists the organization's own roles by title and opens one in its edit form", async () => {
    const user = userEvent.setup();
    renderRoles();
    expect(await screen.findByText("Auditor")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Edit auditor" }));
    const form = await screen.findByRole("dialog", { name: "Edit auditor" });
    expect(await within(form).findByRole("checkbox", { name: en.choice.verb.read }, { timeout: 10_000 })).toBeChecked();
  });

  it("says why the roles cannot be read and reads them again on Retry", async () => {
    const user = userEvent.setup();
    const { reads } = renderRoles({ failFirst: true });
    expect(await screen.findByText(/the role store is restarting/)).toBeInTheDocument();
    // The failure's own Retry is the last button of that name on the page.
    await user.click(screen.getAllByRole("button", { name: en.app.error.retry }).at(-1) as HTMLElement);
    expect(await screen.findByText("Auditor", {}, { timeout: 10_000 })).toBeInTheDocument();
    expect(reads()).toBe(2);
  });

  it("offers a first role on an empty list and shows the proposed change in the form", async () => {
    const user = userEvent.setup();
    const { posted } = renderRoles({ roles: [] });
    expect(await screen.findByText(en.access.projectRoles.empty)).toBeInTheDocument();
    // The empty list's own button, once the permissions say the caller may propose.
    const newRole = () => screen.getAllByRole("button", { name: en.access.projectRoles.new }).at(-1) as HTMLElement;
    await waitFor(() => expect(newRole()).not.toHaveAttribute("aria-disabled"));
    await user.click(newRole());
    // Outside a routed page the form is the component's own.
    await screen.findByText(/A role of the organization/, {}, { timeout: 10_000 });
    await user.type(screen.getByLabelText(new RegExp(en.access.projectRoles.field.name)), "auditor");
    await user.click(screen.getByRole("button", { name: en.form.check }));
    await waitFor(() => expect(checksSoFar()).toContain("POST /api/v1/projects/org/roles"));
    await user.click(screen.getByRole("button", { name: en.access.projectRoles.propose }));

    await waitFor(() => expect(posted).toHaveLength(1));
    expect(posted[0]).toMatchObject({ kind: "Role", metadata: { name: "auditor", namespace: "org" } });
    expect(await screen.findByText(/chg-0000003c/)).toBeInTheDocument();
    await user.click(screen.getAllByRole("button", { name: en.resourceDelete.close }).at(-1) as HTMLElement);
    await waitFor(() => expect(screen.queryByText(/chg-0000003c/)).toBeNull());
  });
});
