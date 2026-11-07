import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";

const IDENTITY = { subject: "e5a2", username: "eva", name: "Eva Nováková", roles: [] };
const EMPTY_LIST = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] };

/** PF-124: a person who reads projects of two Organizations, as `GET /api/v1/projects` lists them. */
const TWO = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    { name: "helsinki", sample: false, organization: "hel" },
    { name: "espoo", sample: false, organization: "hel" },
    { name: "zilina", sample: false, organization: "zilina", title: "Mesto Žilina" },
  ],
  organizations: [
    { name: "hel", title: "City of Helsinki" },
    { name: "zilina", title: { sk: "Mesto Žilina", en: "City of Žilina" } },
  ],
};
const ONE = {
  ...TWO,
  items: TWO.items.filter((item) => item.organization === "hel"),
  organizations: [],
};

function start(projects: object, path: string) {
  window.history.pushState({}, "", path);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const body = url.includes("/auth/me") ? IDENTITY : url.endsWith("/api/v1/projects") ? projects : EMPTY_LIST;
      return Promise.resolve(
        new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }),
      );
    }),
  );
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

const projectItems = async () => {
  await userEvent.setup().click(screen.getByRole("button", { name: "Projects" }));
  const items = (await screen.findAllByRole("menuitem")).map((item) => item.textContent?.trim());
  await userEvent.setup().keyboard("{Escape}");
  return items;
};

describe("the organization switcher (PF-124)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("is not there for a person who sees one Organization", async () => {
    start(ONE, "/projects/helsinki/spaces");
    await screen.findByRole("navigation", { name: "Main navigation" });
    expect(screen.queryByRole("button", { name: /Switch organization/ })).not.toBeInTheDocument();
    expect(await projectItems()).toEqual(["helsinki", "espoo"]);
  });

  it("names the active project's Organization, lists the projects of that one only and switches by ?org=", async () => {
    start(TWO, "/projects/helsinki/spaces");
    // Two copies, the top bar's and the phone sidebar's; CSS shows one, jsdom keeps both.
    const [switcher] = await screen.findAllByRole("button", { name: "Switch organization, now City of Helsinki" });
    expect(switcher).toHaveAttribute("title", expect.stringContaining("for example"));
    expect(await projectItems()).toEqual(["helsinki", "espoo"]);

    const user = userEvent.setup();
    await user.click(switcher);
    const entries = await screen.findAllByRole("menuitem");
    expect(entries.map((entry) => entry.textContent)).toEqual(["City of Helsinkihel", "City of Žilinazilina"]);
    await user.click(screen.getByRole("menuitem", { name: /City of Žilina/ }));

    await waitFor(() => {
      expect(window.location.pathname).toBe("/projects/zilina/spaces");
    });
    expect(new URLSearchParams(window.location.search).get("org")).toBe("zilina");
    expect(screen.getAllByRole("button", { name: "Switch organization, now City of Žilina" })).toHaveLength(2);
    expect(await projectItems()).toEqual(["Mesto Žilinazilina"]);
  });

  it("ignores an ?org= the person does not see and keeps the active project's", async () => {
    start(TWO, "/projects/helsinki/spaces?org=prague");
    expect(
      await screen.findAllByRole("button", { name: "Switch organization, now City of Helsinki" }),
    ).toHaveLength(2);
  });
});
