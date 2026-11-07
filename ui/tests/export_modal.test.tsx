import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { exportQuery } from "../src/components/export/ExportModal";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-viewer"],
};

const ENDPOINTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "public-air", namespace: "banskabystrica" },
      spec: { slug: "mluyob4nz52lok3ssk7pgn5vwt", audience: "public" },
    },
  ],
};

const REVISIONS = {
  items: [
    {
      sha: "8c56954a1f0e2b3c4d5e6f708192a3b4c5d6e7f8",
      message: "Endpoint public-air: add csv",
      author: "Jana Kováčová",
      date: "2026-09-06T16:30:00Z",
    },
  ],
};

function renderEndpoints(
  revisionsStatus = 200,
  exportAnswer?: () => Response,
  projectSpec?: Record<string, unknown>,
) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const url = input instanceof Request ? input.url : input.toString();
    const path = new URL(url, "http://localhost").pathname;
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );
    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/revisions")) {
      return json(revisionsStatus === 200 ? REVISIONS : { status: revisionsStatus }, revisionsStatus);
    }
    if (path.endsWith("/endpoints")) {
      return json(ENDPOINTS);
    }
    if (path === "/api/v1/projects") {
      return json({ items: [{ name: "banskabystrica" }] });
    }
    if (path === "/api/v1/projects/banskabystrica" && projectSpec) {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Project",
        metadata: { name: "banskabystrica", namespace: "org" },
        spec: projectSpec,
        status: { usage: {} },
      });
    }
    if (path.endsWith("/export")) {
      return Promise.resolve(
        exportAnswer
          ? exportAnswer()
          : new Response("PK\u0003\u0004 an archive", {
              status: 200,
              headers: { "content-type": "application/zip" },
            }),
      );
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  vi.stubGlobal("fetch", fetchMock);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

/**
 * Opens the whole project's export where it lives: Administration → Projects, the project's row
 * (UI-87). The top bar offers it no more.
 */
async function openProjectExport(): Promise<void> {
  await userEvent.click(await screen.findByRole("link", { name: en.nav.organization }));
  // Projects is a section of Administration's side navigation (T-4831).
  await userEvent.click(await screen.findByRole("link", { name: en.organization.tab.projects }));
  const table = await screen.findByRole("table", { name: en.organization.projects.caption });
  const row = (await within(table).findByRole("link", { name: "banskabystrica" })).closest("tr") as HTMLElement;
  await userEvent.click(within(row).getByRole("button", { name: en.organization.projects.export }));
}

const downloadButton = () => screen.getByRole("button", { name: en.export.download });

/**
 * Clicks Download and answers the URL the modal asked for.
 *
 * The button fetches and checks the answer before saving it, so what proves the selection is the
 * request the modal made — a link's `href` proved only what the browser would have been handed
 * (MF-16, T-1487).
 */
async function downloaded(fetchMock: ReturnType<typeof vi.fn>): Promise<string> {
  await userEvent.click(downloadButton());
  const call = await waitFor(() => {
    const found = fetchMock.mock.calls
      .map(([input]) => (input instanceof Request ? input.url : String(input)))
      .find((url) => url.includes("/export?"));
    expect(found, "the modal asked for the export").toBeTruthy();
    return found as string;
  });
  const asked = new URL(call, "http://localhost");
  return `${asked.pathname}${asked.search}`;
}

/** The file names the browser was asked to save. */
const saved: string[] = [];
let clicks: ReturnType<typeof vi.spyOn>;

describe("export modal", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/endpoints");
    // jsdom has neither object URLs nor a download: what a save looks like here is the name the
    // anchor was given, recorded when it is clicked.
    saved.length = 0;
    vi.stubGlobal("URL", Object.assign(URL, {
      createObjectURL: () => "blob:export",
      revokeObjectURL: () => undefined,
    }));
    clicks = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(function (this: HTMLAnchorElement) {
        if (this.download) {
          saved.push(this.download);
        }
      });
  });

  afterEach(() => {
    clicks.mockRestore();
    vi.restoreAllMocks();
  });

  it("builds the download query from the selection", () => {
    expect(exportQuery("zip", {})).toEqual({ format: "zip" });
    expect(exportQuery("yaml", { plural: "endpoints", name: "public-air" }, "8c56954")).toEqual({
      format: "yaml",
      kinds: "endpoints",
      names: "public-air",
      revision: "8c56954",
    });
  });

  it("offers the whole project as an archive from Administration, and not from the top bar", async () => {
    const fetchMock = renderEndpoints();

    await screen.findByText("public-air");
    expect(screen.queryByRole("button", { name: /Export project/i })).toBeNull();

    await openProjectExport();

    const dialog = await screen.findByRole("dialog");
    // The first and default choice is the whole project, and it says what the file holds (MF-41).
    const whole = within(dialog).getByRole("radio", {
      name: (name) => name.startsWith(en.export.formats.whole),
    });
    expect(whole).toBeChecked();
    expect(within(dialog).getByText(en.export.formats.wholeHelp)).toBeInTheDocument();
    // The project export defaults to the archive, which is what CC-49 promises in one click.
    expect(await downloaded(fetchMock)).toBe(
      "/api/v1/projects/banskabystrica/export?format=zip",
    );
  });

  it("offers_every_form_of_the_whole_project_at_once_with_the_archive_first", async () => {
    // The plain forms used to sit behind a `details` nobody opened, which is also how the arrow
    // keys stopped walking the group (T-1816). All three are on screen; the archive leads.
    const fetchMock = renderEndpoints();

    await openProjectExport();
    const dialog = await screen.findByRole("dialog");
    const radios = within(dialog).getAllByRole("radio");
    expect(radios).toHaveLength(3);
    expect(radios[0]).toBeChecked();
    expect(radios[0].closest("label")).toHaveTextContent(en.export.formats.whole);
    expect(within(dialog).getByText(en.export.formats.json)).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("radio", { name: /^YAML/ }));
    expect(await downloaded(fetchMock)).toBe(
      "/api/v1/projects/banskabystrica/export?format=yaml",
    );
  });

  it("downloads one manifest when the export starts from its row", async () => {
    const fetchMock = renderEndpoints();

    const row = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
    // The row's actions are behind its one menu now (T-2287).
    await userEvent.click(within(row).getByRole("button", { name: /More actions/ }));
    await userEvent.click(await screen.findByRole("menuitem", { name: en.export.action }));

    expect(await downloaded(fetchMock)).toBe(
      "/api/v1/projects/banskabystrica/export?format=yaml&kinds=endpoints&names=public-air",
    );
  });

  it("offers the history as sentences and puts the chosen revision in the URL", async () => {
    const fetchMock = renderEndpoints();

    await openProjectExport();
    const picker = await screen.findByRole("combobox", { name: en.export.revision });
    await screen.findByRole("option", { name: /Endpoint public-air: add csv/ });

    await userEvent.selectOptions(picker, REVISIONS.items[0].sha);
    expect(await downloaded(fetchMock)).toBe(
      `/api/v1/projects/banskabystrica/export?format=zip&revision=${REVISIONS.items[0].sha}`,
    );
  });

  it("says secrets are never in a download", async () => {
    renderEndpoints();
    await openProjectExport();
    expect(await screen.findByText(en.export.secretsNote)).toBeInTheDocument();
  });

  it("still downloads the current revision when the history is unavailable", async () => {
    const fetchMock = renderEndpoints(503);

    await openProjectExport();
    expect(await screen.findByText(en.export.noForge)).toBeInTheDocument();
    expect(await downloaded(fetchMock)).toBe(
      "/api/v1/projects/banskabystrica/export?format=zip",
    );
  });

  /// MF-16: a refused export is not a file.
  it("says why a refused export saved nothing, and keeps the dialog open", async () => {
    const refusal = () =>
      new Response(
        JSON.stringify({
          title: "Forbidden",
          status: 403,
          detail: "you may not export banskabystrica",
        }),
        { status: 403, headers: { "content-type": "application/problem+json" } },
      );
    renderEndpoints(200, refusal);

    await openProjectExport();
    await userEvent.click(downloadButton());

    const said = await screen.findByRole("alert");
    expect(said).toHaveTextContent("you may not export banskabystrica");
    // Nothing was saved, and the dialog is still there to try another revision or format.
    expect(saved).toHaveLength(0);
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("offers_git_for_a_project_in_its_own_repository_without_a_revision", async () => {
    // MF-45: a project whose registry entry names its repository (layout 2) also exports as one
    // git bundle per repository, which is the whole repository at its default branch.
    const fetchMock = renderEndpoints(200, undefined, {
      organizationRef: "bb",
      repository: { name: "banskabystrica" },
      ref: "main",
    });

    await openProjectExport();
    const dialog = await screen.findByRole("dialog");
    const git = await within(dialog).findByRole("radio", {
      name: (name) => name.startsWith(en.export.formats.git),
    });
    expect(within(dialog).getByLabelText(en.export.revision)).toBeInTheDocument();
    await userEvent.click(git);
    expect(within(dialog).queryByLabelText(en.export.revision)).toBeNull();
    expect(await downloaded(fetchMock)).toBe(
      "/api/v1/projects/banskabystrica/export?format=git",
    );
  });

  it("offers_no_git_for_a_project_that_is_a_folder_of_the_organization", async () => {
    const fetchMock = renderEndpoints(200, undefined, { organizationRef: "bb" });

    await openProjectExport();
    const dialog = await screen.findByRole("dialog");
    // The answer that decides it has arrived, and it names no repository.
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.some(
          ([input]) =>
            new URL(input instanceof Request ? input.url : String(input), "http://localhost")
              .pathname === "/api/v1/projects/banskabystrica",
        ),
      ).toBe(true),
    );
    await waitFor(() => expect(within(dialog).getAllByRole("radio")).toHaveLength(3));
    expect(within(dialog).queryByText(en.export.formats.git)).toBeNull();
  });

  it("saves the archive the server answered, then closes", async () => {
    const fetchMock = renderEndpoints();

    await openProjectExport();
    expect(await downloaded(fetchMock)).toContain("format=zip");

    await waitFor(() => expect(saved).toHaveLength(1));
    // The name comes from the server when it gave one, and from the selection otherwise.
    expect(saved[0]).toBe("banskabystrica.zip");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});
