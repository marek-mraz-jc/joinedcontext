import { StrictMode } from "react";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it, vi, beforeEach } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { expectDenied, expectNoRawKeys, expectNoViolations, expectTabOrder } from "./checks";
import { CompleteFlow, SpaceComplete } from "../src/pages/spaces/SpaceComplete";
import { rememberPrefill, takePrefill } from "../src/assistant/state";
import en from "../src/locales/en.json";
import { xlsxFile } from "./xlsxFixture";

vi.mock("../src/auth/AuthProvider", () => ({
  useAuthStatus: () => "authenticated",
  useAuth: () => ({ status: "authenticated", identity: { username: "jana", email: "jana@hel.fi" } }),
  useSignIn: () => () => undefined,
}));

const navigate = vi.fn();

vi.mock("@tanstack/react-router", () => ({
  useNavigate: () => navigate,
  Link: ({ children, to }: { children: React.ReactNode; to: string }) => <a href={to}>{children}</a>,
}));

/**
 * The space field lists the project's spaces (T-2702): that read is answered here, and every other
 * request reaches the test's own stub, so what a test counts on it is still the completion alone.
 */
function withSpaceList(inner: (input: RequestInfo | URL, init?: RequestInit) => unknown) {
  return (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(input instanceof Request ? input.url : String(input), window.location.origin);
    if (url.pathname === "/api/v1/projects/helsinki/spaces") {
      return Promise.resolve(Response.json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", metadata: {}, items: [] }));
    }
    // The page asks whether the role may propose a space (T-4263); here it may.
    if (url.pathname === "/api/v1/projects/helsinki/permissions/me") {
      return Promise.resolve(Response.json({ project: "helsinki", bootstrap: true, grants: [] }));
    }
    return inner(input, init);
  };
}

function renderComponent(strict = false) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const page = (
    <QueryClientProvider client={queryClient}>
      <I18nextProvider i18n={i18n}>
        <SpaceComplete project="helsinki" />
      </I18nextProvider>
    </QueryClientProvider>
  );
  return render(strict ? <StrictMode>{page}</StrictMode> : page);
}

describe("SpaceComplete page", () => {
  beforeEach(async () => {
    vi.restoreAllMocks();
    // The page's own words, not the bundle that happened to still be English (T-1776).
    await i18n.changeLanguage("en");
  });

  it("renders inputs and runs completion when Complete is clicked", async () => {
    const user = userEvent.setup();

    const mockResponse = {
      space: "bikes",
      found: [],
      drafts: [
        {
          kind: "DataModel",
          name: "bikes",
          inferred: true,
          manifest: { kind: "DataModel", metadata: { name: "bikes" } },
          verdict: { ok: true, findings: [], inputDigest: "abc" },
        },
        {
          kind: "DataSource",
          name: "bikes-source",
          inferred: true,
          manifest: { kind: "DataSource", metadata: { name: "bikes-source" } },
          verdict: { ok: true, findings: [], inputDigest: "def" },
        },
      ],
      proposeReady: true,
      lane: "yellow",
      change: null,
    };

    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(mockResponse), {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", withSpaceList(fetchMock));

    renderComponent();

    expect(screen.getByLabelText(/Endpoint URL/i)).toBeInTheDocument();
    const urlInput = screen.getByLabelText(/Endpoint URL/i);
    await user.type(urlInput, "https://example.com/station_status.json");

    const completeBtn = screen.getByRole("button", { name: "Complete" });
    expect(completeBtn).toBeEnabled();
    await user.click(completeBtn);

    await waitFor(() => {
      expect(screen.getByTestId("complete-draft-DataModel")).toBeInTheDocument();
    });
    expect(screen.getByTestId("complete-draft-DataSource")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Propose all/i })).toBeInTheDocument();
  });

  it("reads each draft without kind names: a label, what it is, whether it is new and whether its check passed", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", withSpaceList(fetchMock));
    window.history.replaceState(null, "", "/projects/helsinki/spaces/complete?space=city-bikes");
    const draft = (kind: string, name: string, spec: Record<string, unknown>, ok = true, inferred = true) => ({
      kind,
      name,
      inferred,
      manifest: { kind, metadata: { name }, spec },
      verdict: { ok, findings: ok ? [] : [{ level: "error", path: "spec.http.url", message: "the feed answered 404 Not Found" }], inputDigest: "abc" },
    });
    rememberPrefill("/projects/helsinki/spaces/complete?space=city-bikes", {
      url: "https://gbfs.example.org/station_status.json",
      result: {
        space: "city-bikes",
        found: [],
        drafts: [
          draft("DataModel", "city-bikes", {
            classes: ["BikeStation"],
            // Model Tools lists a class's slots; a hand-written model may declare attributes inline.
            source: "classes:\n  BikeStation:\n    slots: [name]\n    attributes:\n      capacity: {}\n",
          }),
          draft("ContextSpace", "city-bikes", { dataModelRef: { kind: "DataModel", name: "city-bikes" } }, true, false),
          draft("DataSource", "city-bikes-source", { type: "http", http: { url: "https://gbfs.example.org/station_status.json" } }, false),
          draft("Pipeline", "city-bikes-load", { period: "60s", output: { type: "BikeStation" } }),
          draft("Endpoint", "city-bikes-all", { audience: "organization" }),
          draft("Layer", "city-bikes-map", { sourceEndpointRef: "city-bikes-all", entityType: "BikeStation", style: "circle", colorBy: { property: "capacity" } }),
          draft("Dashboard", "city-bikes", { title: "City Bikes map", visibility: "project", pages: [{ layout: "full-map", layers: ["city-bikes-map"] }] }),
        ],
        proposeReady: true,
        lane: "yellow",
        change: null,
      },
    });

    renderComponent();

    const model = await screen.findByTestId("complete-draft-DataModel");
    expect(model).toHaveTextContent("Data model");
    expect(model).toHaveTextContent("Type BikeStation with 2 attributes");
    expect(model).toHaveTextContent("Check passed");
    expect(model).not.toHaveTextContent("DataModel");
    expect(screen.getByTestId("complete-draft-ContextSpace")).toHaveTextContent("Already there");
    const source = screen.getByTestId("complete-draft-DataSource");
    expect(source).toHaveTextContent("Fetches gbfs.example.org");
    expect(source).toHaveTextContent("Check failed");
    expect(source).toHaveTextContent("spec.http.url: the feed answered 404 Not Found");
    expect(screen.getByTestId("complete-draft-Pipeline")).toHaveTextContent("Loads BikeStation every 60s");
    expect(screen.getByTestId("complete-draft-Endpoint")).toHaveTextContent("Readable by every project of the organization");
    // The map over the new endpoint is drafted with the rest (AG-79).
    expect(screen.getByTestId("complete-draft-Layer")).toHaveTextContent("Map layer");
    expect(screen.getByTestId("complete-draft-Layer")).toHaveTextContent("Draws BikeStation on the map, coloured by capacity");
    expect(screen.getByTestId("complete-draft-Dashboard")).toHaveTextContent("A map dashboard with 1 page");
    expect(screen.queryByText(/\b(green|red|Inferred)\b/)).not.toBeInTheDocument();
    // Proposing is the next step, so it is the primary action and says what approval does.
    expect(screen.getByRole("button", { name: /Propose all/i }).className).toContain("bg-primary");
    expect(screen.getByRole("button", { name: "Complete" }).className).not.toContain("bg-primary");
    expect(screen.getByText(/Once a person approves them/)).toBeInTheDocument();
    // One example per form (T-2882): the feed address carries it; the space picker says what it does.
    expect(screen.getByLabelText(/Space name/i)).toHaveAttribute("placeholder", en.picker.search);
  });

  it("opens with the drafts the assistant completed, ready to propose, without running again (AG-73)", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", withSpaceList(fetchMock));
    window.history.replaceState(null, "", "/projects/helsinki/spaces/complete?space=city-bikes");
    rememberPrefill("/projects/helsinki/spaces/complete?space=city-bikes", {
      url: "https://example.com/free_bike_status.json",
      result: {
        space: "city-bikes",
        found: [],
        drafts: [
          {
            kind: "Pipeline",
            name: "city-bikes-load",
            inferred: true,
            manifest: { kind: "Pipeline", metadata: { name: "city-bikes-load" } },
            verdict: { ok: true, findings: [], inputDigest: "abc" },
          },
        ],
        proposeReady: true,
        lane: "yellow",
        change: null,
      },
    });

    renderComponent();

    expect(await screen.findByTestId("complete-draft-Pipeline")).toBeInTheDocument();
    expect(screen.getByLabelText(/Endpoint URL/i)).toHaveValue("https://example.com/free_bike_status.json");
    expect(screen.getByRole("button", { name: /Propose all/i })).toBeEnabled();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("keeps the handed drafts when React renders the page twice before it commits (T-0894)", async () => {
    vi.stubGlobal("fetch", withSpaceList(vi.fn()));
    window.history.replaceState(null, "", "/projects/helsinki/spaces/complete?space=city-bikes");
    rememberPrefill("/projects/helsinki/spaces/complete?space=city-bikes", {
      url: "https://example.com/free_bike_status.json",
      result: {
        space: "city-bikes",
        found: [],
        drafts: [
          {
            kind: "Pipeline",
            name: "city-bikes-load",
            inferred: true,
            manifest: { kind: "Pipeline", metadata: { name: "city-bikes-load" } },
            verdict: { ok: true, findings: [], inputDigest: "abc" },
          },
        ],
        proposeReady: true,
        lane: "yellow",
        change: null,
      },
    });

    // StrictMode renders and mounts twice, as a navigation the dock's own updates interrupt does.
    renderComponent(true);

    expect(await screen.findByTestId("complete-draft-Pipeline")).toBeInTheDocument();
    expect(screen.getByLabelText(/Endpoint URL/i)).toHaveValue("https://example.com/free_bike_status.json");
  });

  it("opens each draft on its kind's page with the draft in hand (T-0771)", async () => {
    const user = userEvent.setup();
    vi.stubGlobal("fetch", withSpaceList(vi.fn()));
    navigate.mockClear();
    const source = "id: https://example.com/bikes\nname: bikes\n";
    const draft = (kind: string, name: string, spec: Record<string, unknown> = {}) => ({
      kind,
      name,
      inferred: true,
      manifest: { kind, metadata: { name }, spec },
      verdict: { ok: true, findings: [], inputDigest: "abc" },
    });
    window.history.replaceState(null, "", "/projects/helsinki/spaces/complete?space=city-bikes");
    rememberPrefill("/projects/helsinki/spaces/complete?space=city-bikes", {
      url: "https://example.com/free_bike_status.json",
      result: {
        space: "city-bikes",
        found: [],
        drafts: [
          draft("DataModel", "city-bikes", { source }),
          draft("DataSource", "city-bikes-feed"),
          draft("Pipeline", "city-bikes-load"),
          draft("Endpoint", "city-bikes-public"),
          draft("Layer", "city-bikes-map"),
          draft("ContextSpace", "city-bikes"),
        ],
        proposeReady: true,
        lane: "yellow",
        change: null,
      },
    });
    renderComponent();

    const open = async (kind: string) =>
      user.click(within(await screen.findByTestId(`complete-draft-${kind}`)).getByRole("button", { name: en.spaces.complete.open }));

    await open("DataModel");
    expect(navigate).toHaveBeenLastCalledWith({ href: "/projects/helsinki/models" });
    expect(takePrefill("/projects/helsinki/models")).toEqual({ source });
    await open("DataSource");
    expect(navigate).toHaveBeenLastCalledWith({ href: "/projects/helsinki/datasources?draft=city-bikes-feed" });
    await open("Pipeline");
    expect(navigate).toHaveBeenLastCalledWith({ href: "/projects/helsinki/pipelines?draft=city-bikes-load" });
    await open("Endpoint");
    expect(navigate).toHaveBeenLastCalledWith({ href: "/projects/helsinki/endpoints?draft=city-bikes-public" });
    // The dashboards and spaces pages read `?draft=` too (JC1-40).
    await open("Layer");
    expect(navigate).toHaveBeenLastCalledWith({ href: "/projects/helsinki/dashboards?draft=city-bikes-map" });
    await open("ContextSpace");
    expect(navigate).toHaveBeenLastCalledWith({ href: "/projects/helsinki/spaces?draft=city-bikes" });
  });
});

/**
 * The UI contract of the page (T-1776, UI-04, UI-15, UI-16, UI-44, UI-48): axe over the form and
 * over the drafts it proposes, Complete refused with its reason on the button rather than greyed
 * out in silence, every control reachable in the order it is read, and four locales that carry
 * the page's own words.
 */
describe("the complete-a-space page against the UI contract", () => {
  beforeEach(async () => {
    vi.restoreAllMocks();
    await i18n.changeLanguage("en");
  });

  function stubFetch(body: unknown, status = 200) {
    vi.stubGlobal(
      "fetch",
      withSpaceList(
        vi.fn().mockResolvedValue(
          new Response(JSON.stringify(body), {
            status,
            headers: { "content-type": "application/json" },
          }),
        ),
      ),
    );
  }

  const COMPLETED = {
    space: "bikes",
    found: [],
    drafts: [
      {
        kind: "DataModel",
        name: "bikes",
        inferred: true,
        manifest: { kind: "DataModel", metadata: { name: "bikes" } },
        verdict: { ok: true, findings: [], inputDigest: "abc" },
      },
    ],
    proposeReady: true,
    lane: "yellow",
    change: null,
  };

  it("has no axe violation with the form empty and with the drafts on the screen", async () => {
    const user = userEvent.setup();
    stubFetch(COMPLETED);
    const { container } = renderComponent();

    await expectNoViolations(container);

    await user.type(screen.getByLabelText(en.spaces.complete.url), "https://example.org/x.json");
    await user.click(screen.getByRole("button", { name: en.spaces.complete.action }));
    await screen.findByTestId("complete-draft-DataModel");

    await expectNoViolations(container);
  });

  /// UI-44: with nothing to read from, Complete is refused, keeps its place in the tab order and
  /// says on itself what is missing.
  it("refuses Complete with its reason until there is an address or a file", async () => {
    const user = userEvent.setup();
    stubFetch(COMPLETED);
    renderComponent();

    const complete = screen.getByRole("button", { name: en.spaces.complete.action });
    expectDenied(complete, en.spaces.complete.needInput);

    await user.type(screen.getByLabelText(en.spaces.complete.url), "https://example.org/x.json");
    expect(complete).toBeEnabled();
    expect(complete).not.toHaveAttribute("aria-disabled");
  });

  it("reaches the space name, the address and the file picker by keyboard in that order", async () => {
    const user = userEvent.setup();
    stubFetch(COMPLETED);
    const { container } = renderComponent();

    const form = screen.getByLabelText(en.spaces.complete.space).closest("div.flex") as HTMLElement;
    await expectTabOrder(user, form);

    // The file input stays native inside the shared drop zone (T-2758), and it is still labelled.
    const files = screen.getByLabelText(en.spaces.complete.files) as HTMLInputElement;
    expect(files.type).toBe("file");
    expect(files.multiple).toBe(true);
    expect(files.closest("[data-testid=file-drop-zone]")).not.toBeNull();
    expect(container.querySelector("input[type=file]:not(.sr-only)")).toBeNull();
  });

  it("takes files dropped on the zone and names them (T-2758)", async () => {
    stubFetch(COMPLETED);
    renderComponent();
    const zone = screen.getByTestId("file-drop-zone");
    const dropped = [
      new File(["id,name\n1,a"], "stations.csv", { type: "text/csv" }),
      new File(["{}"], "extra.json", { type: "application/json" }),
    ];
    fireEvent.drop(zone, { dataTransfer: { files: dropped } });
    expect(await screen.findByText("Chosen: stations.csv, extra.json")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.spaces.complete.action })).not.toHaveAttribute(
      "aria-disabled",
      "true",
    );
  });

  it("reads an Excel workbook, offers its sheets and completes from the chosen sheet's CSV (JC1-07)", async () => {
    const user = userEvent.setup();
    const op = vi.fn().mockResolvedValue(Response.json(COMPLETED));
    vi.stubGlobal("fetch", withSpaceList(op));
    renderComponent();
    const file = xlsxFile("city.xlsx", {
      Stations: [["name", "opened"], ["Kamppi", { date: 46086 }]],
      Stops: [["code", "lines"], ["H0001", 12]],
    });
    await user.upload(screen.getByLabelText(en.spaces.complete.files), file);
    expect(await screen.findByText("Chosen: city.xlsx")).toBeInTheDocument();
    expect(screen.getByText(en.spaces.complete.filesHint)).toBeInTheDocument();
    await user.selectOptions(screen.getByRole("combobox", { name: "Sheet" }), "Stops");
    await user.click(screen.getByRole("button", { name: en.spaces.complete.action }));
    await screen.findByTestId("complete-draft-DataModel");
    const request = op.mock.calls[0]?.[0] as Request;
    const body = JSON.parse(await request.clone().text()) as { input?: { files?: unknown }; files?: unknown };
    expect(body.input?.files ?? body.files).toEqual([{ name: "Stops.csv", content: "code,lines\r\nH0001,12\r\n" }]);
  });

  describe("one table, one go (T-6445)", () => {
    const LINKML = `id: https://example.org/models/KulturneMiesta
name: KulturneMiesta
imports: [ngsi-ld-core]
classes:
  KulturneMiesta:
    is_a: Entity
    slots: [N_zov, Zemepisn_rka, Zemepisn_d_ka]
slots:
  N_zov: { range: string, required: true, title: { en: Názov } }
  Zemepisn_rka: { range: float, required: true, title: { en: Zemepisná šírka } }
  Zemepisn_d_ka: { range: float, required: true, title: { en: Zemepisná dĺžka } }
`;
    const SHEETS = {
      Obce: [["Obec"], ["Detva"]],
      "Kultúrne miesta": [
        ["Názov", "Zemepisná šírka", "Zemepisná dĺžka"],
        ["Dom kultúry", 48.54, 19.41],
        ["Štadión", 48.55, 19.42],
      ],
    };
    const change = (phase: string) => ({
      kind: "Change",
      metadata: { name: "chg-9" },
      spec: {},
      status: { phase, lane: "red" },
      summary: { key: "change.summary.create", params: { kind: "ContextSpace", name: "miesta" } },
      author: { email: "aino@hel.fi" },
    });
    const completed = (proposed: boolean) => ({
      space: "miesta",
      found: [],
      drafts: [{ kind: "DataModel", name: "miesta", inferred: true, manifest: { kind: "DataModel", spec: { classes: ["KulturneMiesta"], source: LINKML } }, verdict: { ok: true, findings: [], inputDigest: "a" } }],
      proposeReady: true,
      lane: "red",
      change: proposed ? change("PendingApproval") : null,
    });

    /** A dev that answers the op, the Change, the grant and the write; `at` moves it along. */
    function world(): { at: { phase: string; grant: string }; ops: Record<string, unknown>[]; written: unknown[][]; live: boolean } {
      const dev = { at: { phase: "", grant: "" }, ops: [] as Record<string, unknown>[], written: [] as unknown[][], live: false };
      const at = dev.at;
      at.phase = "PendingApproval";
      const { ops, written } = dev;
      vi.stubGlobal(
        "fetch",
        withSpaceList(async (input, init) => {
          const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
          const path = new URL(request.url).pathname;
          if (path.endsWith("/ops/jc_space_complete")) {
            const body = (await request.clone().json()) as Record<string, unknown>;
            ops.push(body);
            return Response.json(dev.live ? { ...completed(false), proposeReady: false, alreadyLive: true } : completed(body.propose === true));
          }
          if (path.endsWith("/changes/chg-9/approve")) {
            ops.push({ approved: await request.clone().text() });
            at.phase = "Merged";
            return Response.json({ ...change("Merged"), status: { phase: "Merged", lane: "red" } });
          }
          if (path.endsWith("/changes/chg-9")) return Response.json(change(at.phase));
          if (path.endsWith("/policies/miesta-upload")) {
            return at.grant ? Response.json({ kind: "Policy", metadata: { name: "miesta-upload" }, spec: {}, status: { phase: at.grant } }) : new Response(null, { status: 404 });
          }
          if (path === "/api/endpoint/miestaslug/ngsi-ld/v1/entityOperations/upsert") {
            written.push((await request.clone().json()) as unknown[]);
            return new Response(null, { status: 204 });
          }
          if (path.endsWith("/endpoints")) {
            return Response.json({ kind: "List", metadata: {}, items: [{ kind: "Endpoint", metadata: { name: "miesta-all" }, spec: { contextSpaceRef: "miesta", slug: "miestaslug" }, status: { phase: "Live" } }] });
          }
          if (path.endsWith("/organizations")) return Response.json({ items: [{ kind: "Organization", metadata: { name: "hel" }, spec: { domain: "hel.fi" } }] });
          return new Response(null, { status: 404 });
        }),
      );
      return dev;
    }

    async function proposed(user: ReturnType<typeof userEvent.setup>): Promise<void> {
      await user.upload(screen.getByLabelText(en.spaces.complete.files), xlsxFile("mesto.xlsx", SHEETS));
      await user.selectOptions(await screen.findByRole("combobox", { name: "Sheet" }), "Kultúrne miesta");
      await user.click(screen.getByRole("button", { name: en.spaces.complete.action }));
      await user.click(await screen.findByRole("button", { name: en.spaces.complete.proposeAll }));
      await screen.findByRole("button", { name: "Write 2 rows" });
    }

    it("asks for the Endpoint and the grant, then loads every row once the grant is live", async () => {
      const user = userEvent.setup();
      const dev = world();
      renderComponent();
      await proposed(user);
      expect(dev.ops[0]).toMatchObject({ upload: true, typeName: "KulturneMiesta", propose: false });
      expect(screen.queryByRole("button", { name: en.spaces.complete.proposeAll })).toBeNull();

      // Approved and merged is not yet live: the grant is what lets the rows in.
      const write = screen.getByRole("button", { name: "Write 2 rows" });
      expect(write).toHaveAttribute("aria-disabled", "true");
      dev.at.phase = "Merged";
      await waitFor(() => expect(screen.queryByText("Review it in Approvals")).toBeNull(), { timeout: 8_000 });
      expect(write).toHaveAttribute("aria-disabled", "true");
      dev.at.grant = "Live";
      await waitFor(() => expect(write).not.toHaveAttribute("aria-disabled", "true"), { timeout: 12_000 });

      await user.click(write);
      expect(await screen.findByText("2 rows written, none refused.")).toBeInTheDocument();
      const rows = dev.written[0] as { id: string; N_zov: { value: string }; Zemepisn_rka: { value: number }; location: { value: { coordinates: number[] } } }[];
      expect(rows).toHaveLength(2);
      expect(rows[0]).toMatchObject({ N_zov: { value: "Dom kultúry" }, Zemepisn_rka: { value: 48.54 }, location: { value: { coordinates: [19.41, 48.54] } } });

      // Done: nothing invites a second copy, and the next step is on the page.
      expect(screen.queryByRole("button", { name: "Write 2 rows" })).toBeNull();
      expect(screen.getByRole("link", { name: "Open the space" })).toBeInTheDocument();
    }, 30_000);

    it("runs in the dock on the table attached there, with no feed address to fill", async () => {
      world();
      const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
      render(
        <QueryClientProvider client={queryClient}>
          <I18nextProvider i18n={i18n}>
            <CompleteFlow project="helsinki" file={xlsxFile("mesto.xlsx", SHEETS)} inDock />
          </I18nextProvider>
        </QueryClientProvider>,
      );
      expect(await screen.findByRole("combobox", { name: "Sheet" })).toBeInTheDocument();
      expect(screen.queryByLabelText(en.spaces.complete.url)).toBeNull();
    });

    it("shows the drafted model with a value of the file, and proposes a corrected type as the model file", async () => {
      const user = userEvent.setup();
      const dev = world();
      renderComponent();
      await user.upload(screen.getByLabelText(en.spaces.complete.files), xlsxFile("mesto.xlsx", SHEETS));
      await user.selectOptions(await screen.findByRole("combobox", { name: "Sheet" }), "Kultúrne miesta");
      await user.click(screen.getByRole("button", { name: en.spaces.complete.action }));
      const review = await screen.findByTestId("model-review");
      expect(review).toHaveTextContent("Dom kultúry");
      await user.selectOptions(within(review).getByLabelText("Zemepisná šírka"), "string");
      await user.click(screen.getByRole("button", { name: en.spaces.complete.proposeAll }));
      await screen.findByRole("button", { name: "Write 2 rows" });
      const files = dev.ops[1].files as { name: string; content: string }[];
      const model = files.find((f) => f.name === "miesta.linkml.yaml");
      expect(model?.content).toMatch(/Zemepisn_rka: \{ range: string/);
      expect(screen.queryByTestId("model-review")).toBeNull();
    }, 30_000);

    it("offers an approver the decision in place, the name typed back on a red lane", async () => {
      const user = userEvent.setup();
      const dev = world();
      renderComponent();
      await proposed(user);
      const approve = await screen.findByRole("button", { name: en.approvals.approve });
      expect(approve).toHaveAttribute("aria-disabled", "true");
      await user.type(screen.getByLabelText(en.approvals.confirmLabel), "miesta");
      await user.click(approve);
      await waitFor(() => expect(dev.ops.some((op) => "approved" in op)).toBe(true));
      await waitFor(() => expect(screen.queryByRole("button", { name: en.approvals.approve })).toBeNull(), { timeout: 8_000 });
    }, 30_000);

    it("tells someone who may not approve who decides, and offers no button", async () => {
      const user = userEvent.setup();
      world();
      const answer = globalThis.fetch;
      vi.stubGlobal("fetch", (input: RequestInfo | URL, init?: RequestInit) => {
        const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
        return new URL(request.url).pathname.endsWith("/permissions/me")
          ? Promise.resolve(Response.json({ project: "helsinki", bootstrap: false, grants: [{ role: "editor", scope: "project:helsinki", rule: { kinds: ["ContextSpace", "DataModel", "Endpoint", "Policy"], verbs: ["read", "propose"] } }] }))
          : answer(input, init);
      });
      renderComponent();
      await proposed(user);
      expect(await screen.findByText(en.changes.whoApproves)).toBeInTheDocument();
      expect(screen.getByText(en.changes.waitingApprover)).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: en.approvals.approve })).toBeNull();
    }, 30_000);

    it("keeps the model unsent when the person corrected nothing", async () => {
      const user = userEvent.setup();
      const dev = world();
      renderComponent();
      await proposed(user);
      expect((dev.ops[1].files as { name: string }[]).some((f) => f.name.endsWith(".linkml.yaml"))).toBe(false);
    }, 30_000);

    it("after a 504 the same Propose is answered the open Change, one proposal on the page", async () => {
      const user = userEvent.setup();
      const dev = world();
      const answer = globalThis.fetch;
      let cut = true;
      vi.stubGlobal("fetch", async (input: RequestInfo | URL, init?: RequestInit) => {
        const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
        if (new URL(request.url).pathname.endsWith("/ops/jc_space_complete") && cut && (await request.clone().json() as { propose?: boolean }).propose) {
          cut = false;
          return new Response("gateway timeout", { status: 504 });
        }
        return answer(input, init);
      });
      renderComponent();
      await user.upload(screen.getByLabelText(en.spaces.complete.files), xlsxFile("mesto.xlsx", SHEETS));
      await user.selectOptions(await screen.findByRole("combobox", { name: "Sheet" }), "Kultúrne miesta");
      await user.click(screen.getByRole("button", { name: en.spaces.complete.action }));
      await user.click(await screen.findByRole("button", { name: en.spaces.complete.proposeAll }));
      expect(await screen.findByRole("alert")).toBeInTheDocument();
      await user.click(screen.getByRole("button", { name: en.spaces.complete.proposeAll }));
      await screen.findByRole("button", { name: "Write 2 rows" });
      expect(screen.queryByRole("alert")).toBeNull();
      expect(screen.getAllByRole("link", { name: en.changes.review })).toHaveLength(1);
      expect(dev.ops.filter((op) => op.propose === true)).toHaveLength(1);
    }, 30_000);

    it("loads the rows at once when the space already serves this model, and proposes nothing", async () => {
      const user = userEvent.setup();
      const dev = world();
      dev.live = true;
      renderComponent();
      await user.upload(screen.getByLabelText(en.spaces.complete.files), xlsxFile("mesto.xlsx", SHEETS));
      await user.selectOptions(await screen.findByRole("combobox", { name: "Sheet" }), "Kultúrne miesta");
      await user.click(screen.getByRole("button", { name: en.spaces.complete.action }));
      const write = await screen.findByRole("button", { name: "Write 2 rows" });
      expect(screen.queryByRole("button", { name: en.spaces.complete.proposeAll })).toBeNull();
      await waitFor(() => expect(write).not.toHaveAttribute("aria-disabled", "true"), { timeout: 12_000 });
    }, 30_000);

    it("keeps a table with a README beside it a feed: no upload, and Complete says the source fills it", async () => {
      const user = userEvent.setup();
      const dev = world();
      renderComponent();
      await user.upload(screen.getByLabelText(en.spaces.complete.files), [new File(["a,b\n1,2\n"], "stations.csv"), new File(["# feed"], "README.md")]);
      await screen.findByText("Chosen: stations.csv, README.md");
      await user.click(screen.getByRole("button", { name: en.spaces.complete.action }));
      await screen.findByText(en.spaces.complete.onApproval);
      expect(dev.ops[0]).not.toHaveProperty("upload");
    });

    it("refuses 11 MB before anything is read or sent (DM-55)", async () => {
      const user = userEvent.setup();
      const op = vi.fn();
      vi.stubGlobal("fetch", withSpaceList(op));
      renderComponent();
      const big = new File([new Uint8Array(11 * 1024 * 1024)], "big.xlsx");
      await user.upload(screen.getByLabelText(en.spaces.complete.files), big);
      expect(await screen.findByRole("alert")).toHaveTextContent("more than 10 MB");
      expect(screen.queryByText("Chosen: big.xlsx")).toBeNull();
      expect(op).not.toHaveBeenCalled();
    });
  });

  it("says a damaged workbook is unreadable and sends nothing", async () => {
    const user = userEvent.setup();
    const op = vi.fn();
    vi.stubGlobal("fetch", withSpaceList(op));
    renderComponent();
    await user.upload(screen.getByLabelText(en.spaces.complete.files), new File(["not a zip"], "broken.xlsx"));
    expect(await screen.findByRole("alert")).toHaveTextContent("not an Excel workbook");
    expect(op).not.toHaveBeenCalled();
  });

  it.each(SUPPORTED_LOCALES)("writes the page in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    stubFetch(COMPLETED);
    const { container } = renderComponent();

    expect(
      screen.getByRole("heading", { name: i18n.t("spaces.complete.title"), level: 1 }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText(i18n.t("spaces.complete.url"))).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: i18n.t("spaces.complete.action") }),
    ).toBeInTheDocument();
    expectNoRawKeys(container);
  });

  it("says the page in Slovak rather than leaving it in English", async () => {
    await i18n.changeLanguage("sk");
    stubFetch(COMPLETED);
    renderComponent();

    // Nine of this page's strings stood in the sk, cs and de bundles as their English originals.
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Doplniť tento priestor");
    expect(screen.getByRole("button", { name: "Doplniť" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Complete" })).toBeNull();
  });
});
