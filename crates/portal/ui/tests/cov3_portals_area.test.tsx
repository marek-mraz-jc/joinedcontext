// Portals area, remaining branches: retry buttons of the list and the editor, a multi-Organization list searched, taken names, a double submit.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import en from "../src/locales/en.json";
import i18n from "../src/i18n";
import { jsonResponse, list, problem, renderRoute } from "./pageHarness";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ rule: { kinds: ["Portal"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }],
};
const ORG = (organization: string) => ({ organization, hosts: [], opendata: `https://opendata.${organization}.sk`, sites: [], bases: [], profiles: [] });
const ONE = { items: [ORG("bbsk")] };
const TWO = { items: [ORG("bbsk"), ORG("ke")] };
const HOSTS = "/api/v1/organization/portal-hosts";
const PORTALS = "/api/v1/projects/org/portals";

function portal(organization: string, path: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Portal",
    metadata: { name: `${organization}-${path}`, namespace: "org" },
    spec: { organizationRef: organization, path, title: { en: `${organization} ${path}` }, languages: ["en"], sources: [{ name: "s1" }] },
  };
}

describe("PortalsArea, more branches", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("reads the hosts of the editor again after a failed read", async () => {
    let healthy = false;
    await renderRoute({
      path: "/portals/new",
      permissions: ADMIN,
      answer: (path) => {
        if (path !== HOSTS) return undefined;
        return !healthy ? problem(500, "hosts are down") : jsonResponse(ONE);
      },
    });
    const retry = await screen.findByRole("button", { name: "Retry" }, { timeout: 20000 });
    healthy = true;
    fireEvent.click(retry);
    expect(await screen.findByLabelText(new RegExp(`^${en.portals.field.path}`))).toBeInTheDocument();
  });

  it("reads the hosts of the list again after a failed read", async () => {
    let healthy = false;
    await renderRoute({
      path: "/portals",
      permissions: ADMIN,
      answer: (path) => {
        if (path !== HOSTS) return undefined;
        return !healthy ? problem(500, "hosts are down") : jsonResponse(ONE);
      },
    });
    const retry = await screen.findByRole("button", { name: "Retry" }, { timeout: 20000 });
    healthy = true;
    fireEvent.click(retry);
    expect(await screen.findByText("No portal yet")).toBeInTheDocument();
  });

  it("reads the portals of the list again after a failed read", async () => {
    let healthy = false;
    await renderRoute({
      path: "/portals",
      permissions: ADMIN,
      answer: (path) => {
        if (path === HOSTS) return jsonResponse(ONE);
        if (path !== PORTALS) return undefined;
        return !healthy ? problem(500, "portals are down") : jsonResponse(list([portal("bbsk", "mobility")]));
      },
    });
    const retry = await screen.findByRole("button", { name: "Retry" }, { timeout: 20000 });
    healthy = true;
    fireEvent.click(retry);
    expect(await screen.findByRole("link", { name: "bbsk mobility" })).toBeInTheDocument();
  });

  it("shows the Organization column of several Organizations and searches every column", async () => {
    const user = userEvent.setup();
    await renderRoute({
      path: "/portals",
      permissions: ADMIN,
      answer: (path) => {
        if (path === HOSTS) return jsonResponse(TWO);
        if (path === PORTALS) return jsonResponse(list([portal("bbsk", "mobility"), portal("ke", "schools")]));
        return undefined;
      },
    });
    const table = await screen.findByRole("table", { name: en.portals.list.title });
    expect(within(table).getByRole("columnheader", { name: /Organization/ })).toBeInTheDocument();
    expect(within(table).getByText("ke")).toBeInTheDocument();
    await user.type(screen.getByRole("searchbox"), "zzz-nothing");
    expect(await screen.findByText("No rows match the search and filters.")).toBeInTheDocument();
  });

  it("reads a stored portal again after a failed read", async () => {
    let healthy = false;
    await renderRoute({
      path: "/portals/bbsk-mobility",
      permissions: ADMIN,
      answer: (path, request) => {
        if (path === HOSTS) return jsonResponse(ONE);
        if (path === `${PORTALS}/bbsk-mobility` && request.method === "GET") {
          return !healthy ? problem(500, "store is down") : jsonResponse(portal("bbsk", "mobility"));
        }
        return undefined;
      },
    });
    const retry = await screen.findByRole("button", { name: "Retry" }, { timeout: 20000 });
    healthy = true;
    fireEvent.click(retry);
    expect(await screen.findByLabelText(en.portals.field.instanceName)).toBeInTheDocument();
  });

  it("refuses a name another portal of the Organization already holds", async () => {
    const user = userEvent.setup();
    await renderRoute({
      path: "/portals/new",
      permissions: ADMIN,
      answer: (path) => {
        if (path === HOSTS) return jsonResponse(ONE);
        if (path === PORTALS) return jsonResponse(list([portal("bbsk", "mobility")]));
        return undefined;
      },
    });
    const path = await screen.findByLabelText(new RegExp(`^${en.portals.field.path}`));
    await user.type(path, "mobility");
    await user.click(screen.getByRole("button", { name: en.portals.editor.create }));
    expect(path).toHaveAttribute("aria-invalid", "true");
  });

  it("unticks and ticks a language", async () => {
    await renderRoute({
      path: "/portals/new",
      permissions: ADMIN,
      answer: (path) => (path === HOSTS ? jsonResponse(ONE) : undefined),
    });
    const czech = await screen.findByRole("checkbox", { name: "Czech" });
    expect(czech).not.toBeChecked();
    fireEvent.click(czech);
    expect(czech).toBeChecked();
    fireEvent.click(czech);
    expect(czech).not.toBeChecked();
  });

  it("sends one proposal when the form is submitted again while it is pending", async () => {
    const user = userEvent.setup();
    let puts = 0;
    await renderRoute({
      path: "/portals/bbsk-mobility",
      permissions: ADMIN,
      answer: (path, request) => {
        if (path === HOSTS) return jsonResponse(ONE);
        if (path === `${PORTALS}/bbsk-mobility` && request.method === "GET") return jsonResponse(portal("bbsk", "mobility"));
        if (path === `${PORTALS}/bbsk-mobility` && request.method === "PUT") {
          puts += 1;
          return new Promise<Response>(() => undefined);
        }
        return undefined;
      },
    });
    const save = await screen.findByRole("button", { name: en.portals.editor.save });
    await user.click(save);
    await waitFor(() => expect(puts).toBe(1));
    const form = save.closest("form") as HTMLFormElement;
    fireEvent.submit(form);
    fireEvent.submit(form);
    expect(puts).toBe(1);
  });
});
