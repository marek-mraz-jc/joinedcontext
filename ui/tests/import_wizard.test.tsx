/**
 * Importing a bundle another instance exported (T-0217, MF-20…MF-24, MF-33, MF-42).
 *
 * The page's whole argument is that nothing is proposed before a person has read what the import
 * would do, and that a credential written in the open never leaves the browser.
 */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRoute, createRouter } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import { ImportPage, importForm } from "../src/pages/import/ImportPage";
import { pastedSecrets } from "../src/api/pastedSecrets";
import { expectNoRawKeys, expectNoViolations, expectTabOrder } from "./checks";

const REPORT = {
  created: ["ContextSpace/ovzdusie", "Endpoint/public-air"],
  replaced: [],
  skipped: ["DataModel/air-quality"],
  renamed: { "Pipeline/aq-ingest": "Pipeline/aq-ingest-2" },
  nativeFiles: 3,
  lane: "yellow",
  source: "https://git.example.sk/bb/org",
  verified: [
    { path: "projects/x/spaces/ovzdusie/space.yaml", equal: true },
    { path: "projects/x/endpoints/public-air.yaml", equal: false },
  ],
  needs: [
    {
      kind: "secret",
      where: "DataSource/aq-mqtt spec.secrets",
      why: "the value behind this reference stays in the origin's secret store; set it here",
      link: "/projects/banskabystrica/datasources",
    },
    {
      kind: "person",
      where: "RoleBinding/stewards spec.subjects[0].user",
      why: "jana@bb.sk is a person of the origin's sign-in; bind someone of this organization",
      link: "/projects/banskabystrica/rolebindings",
    },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-11aa22bb", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 2 } },
};

const BUNDLE = `apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: ovzdusie
spec:
  isSandbox: false
`;

function bundle(text = BUNDLE, name = "export.yaml"): File {
  return new File([text], name, { type: "application/yaml" });
}

function renderPage(options: { refusal?: string; status?: number; grants?: unknown[] } = {}) {
  const posts: { url: string; body: FormData }[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof Request ? input.url : String(input);
    const method = init?.method ?? (input instanceof Request ? input.method : "GET");
    if (url.includes("/import") && method === "POST") {
      // Through the typed client (UI-07) the form travels on the Request itself.
      posts.push({ url, body: (init?.body as FormData | undefined) ?? (await (input as Request).clone().formData()) });
      if (options.refusal) {
        const status = options.status ?? 400;
        return new Response(
          JSON.stringify({ status, title: status === 400 ? "Bad Request" : "Service Unavailable", detail: options.refusal }),
          { status, headers: { "Content-Type": "application/problem+json" } },
        );
      }
      const dry = url.includes("dryRun=All");
      return new Response(JSON.stringify(dry ? REPORT : CHANGE), {
        status: dry ? 200 : 202,
        headers: { "Content-Type": "application/json" },
      });
    }
    if (url.includes("/permissions/me") && options.grants) {
      return new Response(JSON.stringify({ grants: options.grants }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }
    return new Response(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  });
  vi.stubGlobal("fetch", fetchMock);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // The change notice links to the approval, so the page needs a router around it.
  const rootRoute = createRootRoute();
  const home = createRoute({
    getParentRoute: () => rootRoute,
    path: "/",
    component: () => (
      <QueryClientProvider client={client}>
        <ImportPage project="banskabystrica" />
      </QueryClientProvider>
    ),
  });
  const approval = createRoute({
    getParentRoute: () => rootRoute,
    path: "/projects/$project/approvals/$id",
    component: () => <p>approval page</p>,
  });
  const router = createRouter({ routeTree: rootRoute.addChildren([home, approval]) });
  const view = render(
    <I18nextProvider i18n={i18n}>
      <RouterProvider router={router} />
    </I18nextProvider>,
  );
  return { posts, user: userEvent.setup(), container: view.container };
}

describe("the import upload (MF-20)", () => {
  // T-2758: the bundle is dropped on the shared zone, not the browser's own file control.
  it("takes a bundle dropped on the zone and names it", async () => {
    renderPage();
    const zone = await screen.findByTestId("file-drop-zone");
    fireEvent.drop(zone, { dataTransfer: { files: [bundle()] } });
    expect(await within(zone).findByText(/^Chosen: /)).toBeInTheDocument();
    expect(document.querySelector("input[type=file]:not(.sr-only)")).toBeNull();
  });

  it("carries the chosen bundle under `file` and the options as fields", () => {
    const file = bundle();
    const form = importForm(file, " zvolen ", "zvolen.sk", "rename");
    expect(form.get("file")).toBe(file);
    expect(form.get("targetNamespace")).toBe("zvolen");
    expect(form.get("orgDomain")).toBe("zvolen.sk");
    expect(form.get("conflictPolicy")).toBe("rename");
  });

  it("leaves out a namespace and a domain nobody typed", () => {
    const form = importForm(bundle(), "  ", "", "fail");
    expect([...form.keys()]).toEqual(["file", "conflictPolicy"]);
  });
});

describe("the import wizard", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("checks before it proposes, and says what the import would do", async () => {
    const { posts, user } = renderPage();

    await user.upload(await screen.findByLabelText(en.import.file), bundle());
    // Nothing is proposed from an unchecked bundle (MF-33).
    expect(screen.getByRole("button", { name: en.import.propose })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("status")).toHaveTextContent(en.import.checkFirst);

    await user.click(screen.getByRole("button", { name: en.import.check }));

    const report = (await screen.findByRole("heading", { name: en.import.report.title }))
      .closest("section") as HTMLElement;
    expect(within(report).getByText(/^2 created · 0 replaced · 1 left alone · 1 renamed · 3 files/))
      .toBeInTheDocument();
    // The names, not only the counts: what was created and what was left alone.
    await user.click(within(report).getByText(/^2 created$/));
    expect(within(report).getByText("ContextSpace/ovzdusie")).toBeInTheDocument();
    expect(within(report).getByText("Pipeline/aq-ingest → Pipeline/aq-ingest-2", { exact: false }))
      .toBeInTheDocument();
    // MF-42: the checksum count as it is, one equal of two.
    expect(screen.getByText(/1 of 2 files equal/)).toBeInTheDocument();
    // CC-84: what the copy cannot carry, as a checklist with where each thing is set.
    const needs = (screen.getByRole("heading", { name: en.import.report.needsTitle })
      .closest("section") as HTMLElement);
    expect(within(needs).getByRole("checkbox", { name: "DataSource/aq-mqtt spec.secrets" })).not.toBeChecked();
    expect(within(needs).getByText(en.import.report.need.person)).toBeInTheDocument();
    expect(within(needs).getAllByRole("link", { name: en.import.report.needSet })[0]).toHaveAttribute(
      "href",
      "/projects/banskabystrica/datasources",
    );

    expect(posts).toHaveLength(1);
    expect(posts[0].url).toContain("dryRun=All");

    await user.click(screen.getByRole("button", { name: en.import.propose }));
    expect(await screen.findByText(CHANGE.metadata.name)).toBeInTheDocument();
    expect(posts).toHaveLength(2);
    expect(posts[1].url).not.toContain("dryRun");
  });

  it("sends the namespace, the domain and the conflict policy the person chose", async () => {
    const { posts, user } = renderPage();

    await user.upload(await screen.findByLabelText(en.import.file), bundle());
    const namespace = screen.getByLabelText(en.import.namespace);
    await user.clear(namespace);
    await user.type(namespace, "zvolen");
    await user.type(screen.getByLabelText(en.import.orgDomain), "zvolen.sk");
    await user.selectOptions(screen.getByLabelText(en.import.policy), "rename");
    await user.click(screen.getByRole("button", { name: en.import.check }));

    await waitFor(() => expect(posts).toHaveLength(1));
    expect(posts[0].body.get("targetNamespace")).toBe("zvolen");
    expect(posts[0].body.get("orgDomain")).toBe("zvolen.sk");
    expect(posts[0].body.get("conflictPolicy")).toBe("rename");
    // jsdom's File does not survive the Request's own FormData; the builder's test below
    // proves the bundle is what goes under `file`.
    expect(posts[0].body.has("file")).toBe(true);
  });

  /// MF-24: the credential does not leave the browser, and the refusal says which key it was.
  it("refuses a bundle with a credential written in the open, before anything is sent", async () => {
    const { posts, user } = renderPage();

    await user.upload(
      await screen.findByLabelText(en.import.file),
      bundle(`${BUNDLE}  auth:\n    password: hunter2\n`),
    );

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/password/);
    expect(screen.getByRole("button", { name: en.import.check })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("button", { name: en.import.propose })).toHaveAttribute("aria-disabled", "true");
    expect(posts).toHaveLength(0);
  });

  it("shows the Portal's own reason when the bundle is refused", async () => {
    const { user } = renderPage({ refusal: "public-air: literal secret in field 'apiKey'" });

    await user.upload(await screen.findByLabelText(en.import.file), bundle());
    await user.click(screen.getByRole("button", { name: en.import.check }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("public-air: literal secret in field 'apiKey'");
    expect(screen.queryByRole("heading", { name: en.import.report.title })).toBeNull();
  });

  // MF-20, T-1503: a server that fails the check is an answer on the page, not a report and not a
  // way on to propose.
  it("says why the check failed when the server does, and offers nothing to propose", async () => {
    const { user } = renderPage({ refusal: "git forge is not configured", status: 503 });

    await user.upload(await screen.findByLabelText(en.import.file), bundle());
    await user.click(screen.getByRole("button", { name: en.import.check }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("git forge is not configured");
    expect(screen.queryByRole("heading", { name: en.import.report.title })).toBeNull();
    expect(screen.getByRole("button", { name: en.import.propose })).toHaveAttribute("aria-disabled", "true");
    // The check can be asked again once the server is back.
    expect(screen.getByRole("button", { name: en.import.check })).not.toHaveAttribute("aria-disabled");
  });

  it("says a bundle is uploaded rather than fetched, and offers no URL to fetch", async () => {
    renderPage();
    expect(await screen.findByText(en.import.noUrl)).toBeInTheDocument();
    expect(screen.queryByLabelText(/URL/i)).toBeNull();
    // It says what to do, not why the Portal is built so (photo audit T-4159, T-4165).
    expect(en.import.noUrl).toMatch(/^Upload/);
    expect(en.import.noUrl).not.toMatch(/policy|host|connection/i);
  });

  describe("the pre-check itself", () => {
    it("finds a pasted credential whatever key it hides behind", () => {
      expect(pastedSecrets("password: hunter2")).toEqual(["password"]);
      expect(pastedSecrets("  - apiToken: abc123")).toEqual(["apiToken"]);
      expect(pastedSecrets("client_secret: x\napi_key: y")).toEqual(["client_secret", "api_key"]);
    });

    it("leaves a reference and an empty block alone", () => {
      // What a manifest is supposed to carry (CC-06).
      expect(pastedSecrets("secretRef:\n  name: mqtt\n  key: password")).toEqual([]);
      expect(pastedSecrets("password: { name: mqtt, key: password }")).toEqual([]);
      expect(pastedSecrets("password:\n  name: mqtt")).toEqual([]);
      expect(pastedSecrets("# password: hunter2 in a comment is still text\nname: air")).toEqual([]);
    });
  });
});

/**
 * The UI contract of this page (T-1769, UI-04, UI-15, UI-16, UI-44, UI-48): its controls are the
 * shared ones inside a `Field`, axe is clean on both steps, Tab reaches every control in the
 * order they are read, and the four locales carry every string the page shows.
 */
describe("who may import (T-4997)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("shows a viewer the refusal sentence and no drop zone or propose", async () => {
    renderPage({ grants: [{ rule: { kinds: ["*"], verbs: ["read"] } }] });
    expect(await screen.findByText(en.form.mayNotCreate)).toBeInTheDocument();
    expect(screen.queryByLabelText(en.import.file)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.import.propose })).not.toBeInTheDocument();
  });

  it("shows a steward the drop zone and the propose button", async () => {
    renderPage({ grants: [{ rule: { kinds: ["ContextSpace", "Endpoint"], verbs: ["read", "propose"] } }] });
    expect(await screen.findByLabelText(en.import.file)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.import.propose })).toBeInTheDocument();
    expect(screen.queryByText(en.form.mayNotCreate)).not.toBeInTheDocument();
  });
});

describe("the import page against the UI contract", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("has no axe violation before the check and with the report on the screen", async () => {
    await i18n.changeLanguage("en");
    const { user, container } = renderPage();

    await screen.findByLabelText(en.import.file);
    await expectNoViolations(container);

    await user.upload(screen.getByLabelText(en.import.file), bundle());
    await user.click(screen.getByRole("button", { name: en.import.check }));
    await screen.findByRole("heading", { name: en.import.report.title });
    await expectNoViolations(container);
  });

  it("reaches every control of the report by keyboard in the order it is read", async () => {
    await i18n.changeLanguage("en");
    const { user, container } = renderPage();

    await user.upload(await screen.findByLabelText(en.import.file), bundle());
    await user.click(screen.getByRole("button", { name: en.import.check }));
    const needs = (await screen.findByRole("heading", { name: en.import.report.needsTitle }))
      .closest("section") as HTMLElement;

    // Each thing the copy cannot carry is a tick and the link that sets it, both reachable.
    await expectTabOrder(user, needs);

    // The tick is labelled by the text beside it, so clicking that text ticks it (the hand-made
    // input carried an `aria-label` and a separate span, which a pointer could not hit).
    const tick = within(needs).getByRole("checkbox", { name: REPORT.needs[0].where });
    await user.click(within(needs).getByText(REPORT.needs[0].where));
    expect(tick).toBeChecked();
    expect(container).toContainElement(tick);
  });

  it.each(SUPPORTED_LOCALES)("shows no translation key in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    const { user, container } = renderPage();

    const file = await screen.findByLabelText(i18n.t("import.file"));
    await user.upload(file, bundle());
    await user.click(screen.getByRole("button", { name: i18n.t("import.check") }));
    await screen.findByRole("heading", { name: i18n.t("import.report.title") });

    expectNoRawKeys(container);
  });
});
