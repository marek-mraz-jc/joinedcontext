/**
 * T-1780: "Data sources" against the page contract (UI-11, UI-15, UI-16, UI-44, MF-35).
 *
 * `datasources_view.test.tsx` walks the page through the whole application and keeps the forms,
 * the secret references and the proposal. This one mounts the page itself and asserts the frame
 * no axe run had reached: the H1 and the tab, the four states of the list, a table that reads
 * the same with none, one and five hundred rows, the keyboard, four languages, and that a
 * credential is only ever named on the page, never shown.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { DataSourcesPage } from "../src/pages/datasources/DataSourcesPage";
import {
  expectHeadingOutline,
  expectNoAxeViolations,
  inEveryLocale,
  json,
  list,
  problem,
  renderPage,
  tabOrder,
} from "./page_contract";

const PROJECT = "helsinki";

function mqtt(index: number) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataSource",
    metadata: {
      name: `mqtt-${index}`,
      namespace: PROJECT,
      title: { en: `City broker ${index}` },
    },
    spec: {
      type: "mqtt",
      mqtt: {
        urls: ["tls://mqtt.helsinki.fi:8883"],
        topics: ["sensors/aq/+/reading"],
        username: "collector",
        passwordRef: { name: "mqtt-secret", key: "password" },
      },
    },
    status: { phase: "Live" },
  };
}

interface World {
  rows?: number;
  fails?: { status: number; detail: string };
  pending?: boolean;
  items?: unknown[];
}

function renderSources(world: World = {}) {
  const { rows = 1, fails, pending = false, items } = world;
  return renderPage(<DataSourcesPage project={PROJECT} />, {
    path: `/projects/${PROJECT}/datasources`,
    answer: (url, request) => {
      if (!url.pathname.endsWith("/datasources") || request.method !== "GET") return undefined;
      if (pending) return new Promise<Response>(() => undefined);
      if (fails) return problem(fails.status, fails.detail);
      return json(list(items ?? Array.from({ length: rows }, (_, index) => mqtt(index))));
    },
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  window.history.replaceState({}, "", `/projects/${PROJECT}/datasources`);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  document.title = "";
});

describe("the data sources page", () => {
  it("has one H1 and names the page and the project in the tab", async () => {
    const { container } = renderSources();
    expect(
      await screen.findByRole("heading", { level: 1, name: en.datasources.title }),
    ).toBeInTheDocument();
    expect(screen.getByText(en.datasources.lead)).toBeInTheDocument();
    expectHeadingOutline(container);
    await waitFor(() => {
      expect(document.title).toBe(
        `${en.datasources.title} · ${PROJECT} · Helsinki Region Context`,
      );
    });
  });

  // The type of the next source is a labelled control, not a bare select under a line of text.
  it("labels the type of the source it is about to create, inside its form (T-2758)", async () => {
    renderSources();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await screen.findByRole("dialog");
    const type = within(dialog).getByLabelText(en.datasources.field.type);
    expect(type.tagName).toBe("SELECT");
  });

  it("says the list is loading rather than showing it empty", async () => {
    renderSources({ pending: true });
    const table = await screen.findByRole("table", { name: en.datasources.title });
    expect(table).toHaveAttribute("aria-busy", "true");
    expect(screen.queryByText(en.datasources.empty)).not.toBeInTheDocument();
  });

  it("says what a project with no source looks like, and offers the first one", async () => {
    const { container } = renderSources({ rows: 0 });
    expect(await screen.findByText(en.datasources.empty)).toBeInTheDocument();
    expect(screen.getByText(en.datasources.emptyHint)).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: en.datasources.add }).length).toBeGreaterThan(0);
    await expectNoAxeViolations(container);
  });

  // A list that failed is not "this project has no data source".
  it("shows the reason the list failed, with a retry", async () => {
    renderSources({ fails: { status: 503, detail: "The configuration store is not answering." } });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The configuration store is not answering.",
    );
    expect(screen.getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
    expect(screen.queryByText(en.datasources.empty)).not.toBeInTheDocument();
  });

  it("says a refusal in the API's own words", async () => {
    renderSources({ fails: { status: 403, detail: "You may not read this project's sources." } });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "You may not read this project's sources.",
    );
  });

  it.each([0, 1, 500])("draws %i sources without changing how the page reads", async (rows) => {
    const { container } = renderSources({ rows });
    const table = await screen.findByRole("table", { name: en.datasources.title });
    await waitFor(() => {
      expect(table).not.toHaveAttribute("aria-busy");
    });
    expect(table.querySelectorAll("tbody tr")).toHaveLength(Math.max(rows, 1));
    expectHeadingOutline(container);
  });

  // MF-35, PF-50: a credential is named on the page, never shown. The form takes the name and
  // the key of a reference; the value never leaves the secret store.
  it("names a source's credential and never shows one", async () => {
    // The endpoint is public information and is listed; a credential is a reference, so only
    // the secret's name reaches the page. A value that somehow rode along in the manifest —
    // which the API must never send — is still not drawn anywhere (MF-35, PF-50).
    renderSources({
      items: [
        {
          ...mqtt(0),
          spec: {
            type: "mqtt",
            mqtt: {
              ...mqtt(0).spec.mqtt,
              password: "hunter2-should-never-be-here",
            },
          },
        },
      ],
    });
    const table = await screen.findByRole("table", { name: en.datasources.title });
    expect(await within(table).findByText("mqtt-secret")).toBeInTheDocument();
    expect(document.body.textContent).not.toContain("hunter2-should-never-be-here");
    // The key inside the secret is not the page's business either.
    expect(within(table).queryByText("password")).not.toBeInTheDocument();
  });

  it("says a source with no credential has none", async () => {
    renderSources({
      items: [
        {
          ...mqtt(0),
          spec: { type: "http", http: { url: "https://api.helsinki.fi/air", verb: "GET" } },
        },
      ],
    });
    const table = await screen.findByRole("table", { name: en.datasources.title });
    expect(await within(table).findByText(en.datasources.noSecret)).toBeInTheDocument();
  });

  // UI-16: the keyboard reaches the type, the action and each row's actions, and nothing takes
  // focus on arrival.
  it("is reachable by keyboard and takes no focus on arrival", async () => {
    const { container } = renderSources({ rows: 2 });
    const add = (await screen.findAllByRole("button", { name: en.datasources.add }))[0];
    expect(document.activeElement).toBe(document.body);
    const reached = await tabOrder(container, 60);
    expect(reached).toContain(add);
  });

  // The page's one primary action opens the same form the assistant's draft opens.
  it("opens the new-source form from the page's own action", async () => {
    renderSources({ rows: 0 });
    const user = userEvent.setup();
    await user.click((await screen.findAllByRole("button", { name: en.datasources.add }))[0]);
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("says the same things in every language", async () => {
    await inEveryLocale(async (locale) => {
      cleanup();
      renderSources({ rows: 0 });
      expect(
        await screen.findByRole("heading", { level: 1, name: i18n.t("datasources.title") }),
        `the title is missing in ${locale}`,
      ).toBeInTheDocument();
      expect(await screen.findByText(i18n.t("datasources.emptyHint"))).toBeInTheDocument();
    });
  });

  // PF-50: a title out of a manifest is read as text.
  it("renders a source title that arrived as markup as text", async () => {
    renderSources({
      items: [
        { ...mqtt(0), metadata: { ...mqtt(0).metadata, title: { en: "<img src=x onerror=alert(1)>" } } },
      ],
    });
    expect(await screen.findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("has no axe violations with sources listed", async () => {
    const { container } = renderSources({ rows: 3 });
    const table = await screen.findByRole("table", { name: en.datasources.title });
    await waitFor(() => {
      expect(table).not.toHaveAttribute("aria-busy");
    });
    await expectNoAxeViolations(container);
  });

  it("has no axe violations in the failed state", async () => {
    const { container } = renderSources({ fails: { status: 503, detail: "Not answering." } });
    await screen.findByRole("alert");
    await expectNoAxeViolations(container);
  });
});
