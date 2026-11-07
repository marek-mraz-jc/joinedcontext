/**
 * T-3840, DM-81: the Upload CSV page ranks the file against Smart Data Models, proposes the
 * model with the upload grant through `jc_space_complete`, and upserts the rows through the
 * gateway; an own model, a refused proposal and refused rows each say what happened.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { LoadResult, LoadRows, RowsCheck } from "../src/pages/spaces/LoadRows";
import { UploadCsv } from "../src/pages/spaces/UploadCsv";
import { expectNoRawKeys } from "./checks";
import { xlsxFile } from "./xlsxFixture";

vi.mock("../src/auth/AuthProvider", () => ({
  useAuthStatus: () => "authenticated",
  useAuth: () => ({ status: "authenticated", identity: { username: "jana", email: "jana@hel.fi" } }),
  useSignIn: () => () => undefined,
}));

vi.mock("@tanstack/react-router", () => ({
  useNavigate: () => vi.fn(),
  Link: ({ children, to }: { children: React.ReactNode; to: string }) => <a href={to}>{children}</a>,
}));

const LINKML = `id: https://smartdatamodels.org/OffStreetParking
name: OffStreetParking
classes:
  OffStreetParking:
    slots: [name, totalSpotNumber, availableSpotNumber, location]
slots:
  name: { range: string, required: true }
  totalSpotNumber: { range: integer }
  availableSpotNumber: { range: integer }
  location:
    annotations: { ngsi_ld_kind: GeoProperty }
`;

const CATALOGUE = {
  subjects: [
    {
      name: "dataModel.Parking",
      models: [
        { id: "dataModel.Parking/OffStreetParking", name: "OffStreetParking", attributes: ["name", "location", "totalSpotNumber", "availableSpotNumber", "openingHours"] },
      ],
    },
  ],
};

const CSV = "name,totalSpotNumber,availableSpotNumber,lat,lon\nKamppi,500,120,60.17,24.93\n,10,2,60.1,24.9\n";

type Calls = { url: URL; body: unknown }[];

function server(answers: { op?: () => Response; write?: () => Response; catalogue?: () => Promise<Response>; change?: () => Response } = {}): Calls {
  const calls: Calls = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(input instanceof Request ? input.url : String(input), window.location.origin);
      const raw = input instanceof Request ? await input.clone().text() : init?.body;
      calls.push({ url, body: typeof raw === "string" && raw ? JSON.parse(raw) : undefined });
      if (url.pathname.endsWith("/organizations")) return Response.json({ items: [{ kind: "Organization", metadata: { name: "hel" }, spec: { domain: "hel.fi" } }] });
      if (url.pathname.endsWith("/spaces")) return Response.json({ kind: "List", metadata: {}, items: [] });
      if (url.pathname.endsWith("/endpoints")) {
        return Response.json({ kind: "List", metadata: {}, items: [{ kind: "Endpoint", metadata: { name: "parking-all" }, spec: { contextSpaceRef: "parking", slug: "parkingslug" }, status: { phase: "Live" } }] });
      }
      if (url.pathname === "/api/v1/tools/sdm-catalog") return answers.catalogue?.() ?? Response.json(CATALOGUE);
      if (url.pathname === "/api/v1/tools/import-sdm") return Response.json({ linkml: LINKML });
      if (url.pathname.endsWith("/ops/jc_space_complete")) {
        return answers.op?.() ?? Response.json({ space: "parking", found: [], drafts: [], proposeReady: true, lane: "yellow", change: null });
      }
      if (url.pathname.includes("/changes/")) return answers.change?.() ?? new Response(null, { status: 404 });
      if (url.pathname.endsWith("/policies/parking-upload")) return Response.json({ kind: "Policy", metadata: { name: "parking-upload" }, spec: {}, status: { phase: "Live" } });
      if (url.pathname.includes("/entityOperations/")) return answers.write?.() ?? new Response(null, { status: 204 });
      return new Response(null, { status: 404 });
    }),
  );
  return calls;
}

function page(space = "") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <UploadCsv project="helsinki" space={space} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("Upload CSV", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("proposes the ranked Smart Data Models type, then upserts the valid rows and lists the refused one", async () => {
    const calls = server();
    const user = userEvent.setup();
    const { container } = page("parking");
    await user.upload(screen.getByLabelText("Choose a CSV file or an Excel workbook (.xlsx)"), new File([CSV], "garages.csv", { type: "text/csv" }));

    const best = await screen.findByRole("radio", { name: /OffStreetParking/ });
    expect(best).toBeChecked();
    const catalogue = calls.find((c) => c.url.pathname === "/api/v1/tools/sdm-catalog");
    expect(catalogue?.url.searchParams.get("subject")?.split(",")).toHaveLength(8);

    await user.click(screen.getByRole("button", { name: "Propose the change" }));
    await screen.findByText(/rows into parking as OffStreetParking/);
    const op = calls.find((c) => c.url.pathname.endsWith("/ops/jc_space_complete") && (c.body as { propose?: boolean }).propose)?.body as Record<string, unknown>;
    expect(op).toMatchObject({ space: "parking", typeName: "OffStreetParking", upload: true, propose: true });
    expect((op.files as { name: string }[]).map((f) => f.name)).toEqual(["garages.csv", "OffStreetParking.linkml.yaml"]);

    expect(JSON.parse(screen.getByTestId("upload-preview").textContent ?? "")).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: "Write 1 row" }));
    await screen.findByText(/1 row written, 1 refused\./);
    const write = calls.find((c) => c.url.pathname.includes("/entityOperations/"));
    expect(write?.url.pathname).toBe("/api/endpoint/parkingslug/ngsi-ld/v1/entityOperations/upsert");
    expect((write?.body as Record<string, unknown>[])[0]).toMatchObject({
      type: "OffStreetParking",
      location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.93, 60.17] } },
    });
    expect(screen.getByRole("button", { name: "Download the refused rows (CSV)" })).toBeInTheDocument();
    expectNoRawKeys(container);
  });

  it("reads an Excel workbook, offers every sheet and proposes the chosen one as its CSV (JC1-07)", async () => {
    const calls = server();
    const user = userEvent.setup();
    const { container } = page("parking");
    const file = xlsxFile("city.xlsx", {
      Notes: [["author", "remark"], ["Aino", "draft"]],
      Garages: [
        ["name", "totalSpotNumber", "availableSpotNumber", "lat", "lon"],
        ["Kamppi", 500, 120, 60.17, 24.93],
        ["", 10, 2, 60.1, 24.9],
      ],
    });
    await user.upload(screen.getByLabelText("Choose a CSV file or an Excel workbook (.xlsx)"), file);

    const sheet = await screen.findByRole("combobox", { name: "Sheet" });
    expect(screen.getByText(/One sheet is read at a time/)).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Notes (1 row, 2 columns)" })).toBeInTheDocument();
    expect(screen.queryByRole("radio", { name: /OffStreetParking/ })).toBeNull();
    await user.selectOptions(sheet, "Garages");
    expect(await screen.findByRole("radio", { name: /OffStreetParking/ })).toBeChecked();

    await user.click(screen.getByRole("button", { name: "Propose the change" }));
    await screen.findByText(/rows into parking as OffStreetParking/);
    const op = calls.find((c) => c.url.pathname.endsWith("/ops/jc_space_complete") && (c.body as { propose?: boolean }).propose)?.body as { files: { name: string; content: string }[] };
    expect(op.files[0]).toEqual({
      name: "Garages.csv",
      content: "name,totalSpotNumber,availableSpotNumber,lat,lon\r\nKamppi,500,120,60.17,24.93\r\n,10,2,60.1,24.9\r\n",
    });
    await user.click(screen.getByRole("button", { name: "Write 1 row" }));
    await screen.findByText(/1 row written, 1 refused\./);
    expectNoRawKeys(container);
  });

  it("says on screen what keeps Propose out of reach, names the columns read and hints the space", async () => {
    server();
    const user = userEvent.setup();
    page("");
    await user.upload(screen.getByLabelText("Choose a CSV file or an Excel workbook (.xlsx)"), new File([CSV], "__.csv", { type: "text/csv" }));
    expect(await screen.findByText("Columns: name, totalSpotNumber, availableSpotNumber, lat, lon")).toBeInTheDocument();
    expect(screen.getAllByText("Choose a Context Space to propose.").find((line) => line.tagName === "P")).toBeVisible();
    expect(screen.getByText(/An existing space, or a new name in lower case/)).toBeInTheDocument();
    await user.click(screen.getByRole("radio", { name: "My own model" }));
    expect(screen.getAllByText("Choose a type, or write your own type name.").find((line) => line.tagName === "P")).toBeVisible();
  });

  it("names the space and the type after the sheet, draws the drafted model and proposes in one click (T-6567)", async () => {
    const drafted = "id: https://example.org/models/DetskeIhriska\nname: DetskeIhriska\nimports: [linkml:types, ngsi-ld-core]\nclasses:\n  DetskeIhriska:\n    is_a: Entity\n    slots: [Nazov, Bezbarierove]\nslots:\n  Nazov: { range: string }\n  Bezbarierove: { range: boolean }\n";
    const calls = server({
      catalogue: async () => Response.json({ subjects: [] }),
      op: () => Response.json({ space: "detske-ihriska", found: [], drafts: [{ kind: "DataModel", name: "detske-ihriska", manifest: { spec: { source: drafted } } }], proposeReady: true, lane: "yellow", change: null }),
    });
    const user = userEvent.setup();
    page("");
    expect(screen.getByText("What this file becomes")).toBeInTheDocument();
    const file = xlsxFile("priklad.xlsx", { "Detské ihriská": [["Názov", "Zemepisná šírka", "Zemepisná dĺžka", "Bezbariérové"], ["Tulská", 48.73, 19.12, "áno"]] });
    await user.upload(screen.getByLabelText("Choose a CSV file or an Excel workbook (.xlsx)"), file);

    expect(await screen.findByLabelText("Type name")).toHaveValue("DetskeIhriska");
    expect(screen.getByText(/None of the Smart Data Models types fits these columns/)).toBeInTheDocument();
    expect(screen.queryByRole("radio")).toBeNull();
    expect(screen.queryByText("What this file becomes")).toBeNull();
    const diagram = await screen.findByTestId("drafted-model", {}, { timeout: 3_000 });
    expect(diagram).toHaveTextContent("The data model DetskeIhriska");
    expect(diagram).toHaveTextContent("Yes or no");
    expect(diagram).toHaveTextContent("Place");
    const dry = calls.find((c) => c.url.pathname.endsWith("/ops/jc_space_complete"))?.body;
    expect(dry).toMatchObject({ space: "detske-ihriska", typeName: "DetskeIhriska", propose: false });

    await user.click(screen.getByRole("button", { name: "Propose the change" }));
    await screen.findByText(/rows into detske-ihriska as DetskeIhriska/);
    expect(calls.filter((c) => (c.body as { propose?: boolean } | undefined)?.propose === true)).toHaveLength(1);
  });

  it("says who can change it when the space refuses the write (JC1-41)", async () => {
    server({
      write: () =>
        new Response(JSON.stringify({ type: "about:blank", title: "Forbidden", status: 403, detail: "no grant" }), {
          status: 403,
          headers: { "Content-Type": "application/problem+json" },
        }),
    });
    const user = userEvent.setup();
    page("parking");
    await user.upload(screen.getByLabelText("Choose a CSV file or an Excel workbook (.xlsx)"), new File([CSV], "garages.csv", { type: "text/csv" }));
    await screen.findByRole("radio", { name: /OffStreetParking/ });
    await user.click(screen.getByRole("button", { name: "Propose the change" }));
    await user.click(await screen.findByRole("button", { name: "Write 1 row" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Nothing was written: the space parking does not let your role create OffStreetParking entities",
    );
  });

  it("says when the model gives the rows nowhere to go, and offers no write", async () => {
    server();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <LoadRows project="helsinki" space="parking" type="Garage" source={undefined} table={{ columns: ["name"], rows: [{ name: "Kamppi" }] }} change={null} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(screen.getByText("The proposed model declares no fields for this type: there is nothing to write.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Nothing to write" })).toHaveAttribute("aria-disabled", "true");
  });

  it("says what the check found, and shows the first rows as they will be written", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <RowsCheck checked={{ entities: [{ row: 2, entity: { id: "urn:ngsi-ld:Garage:a" } }], rejected: [{ row: 3, reason: "no name" }] }} />
      </I18nextProvider>,
    );
    expect(screen.getByRole("status")).toHaveTextContent("1");
    expect(screen.getByTestId("upload-preview")).toHaveTextContent("urn:ngsi-ld:Garage:a");
  });

  it("names the refusal and the first refused row, and offers their file", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <LoadResult done={{ written: 0, rejected: [{ row: 3, reason: "no name" }], denied: "no grant" }} project="helsinki" space="parking" type="Garage" />
      </I18nextProvider>,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("no grant");
    expect(screen.getByRole("status")).toHaveTextContent("no name");
    expect(screen.getByRole("button", { name: i18n.t("spaces.import.downloadRefused") })).toBeInTheDocument();
  });

  it("holds the write until the proposed change is live", async () => {
    let phase = "PendingApproval";
    const change = () => ({ kind: "Change", metadata: { name: "chg-1" }, spec: {}, status: { phase, lane: "yellow" } });
    const calls = server({
      op: () => Response.json({ space: "parking", found: [], drafts: [], proposeReady: true, lane: "yellow", change: change() }),
      change: () => Response.json(change()),
    });
    const user = userEvent.setup();
    page("parking");
    await user.upload(screen.getByLabelText("Choose a CSV file or an Excel workbook (.xlsx)"), new File([CSV], "garages.csv", { type: "text/csv" }));
    await screen.findByRole("radio", { name: /OffStreetParking/ });
    await user.click(screen.getByRole("button", { name: "Propose the change" }));
    const write = await screen.findByRole("button", { name: "Write 1 row" });
    expect(write).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText("The rows can be written once the change is approved and live.")).toBeInTheDocument();

    // No code sets Applied: a merged Change is as far as the phase goes, and the grant says live.
    phase = "Merged";
    await waitFor(() => expect(screen.getByRole("button", { name: "Write 1 row" })).not.toHaveAttribute("aria-disabled", "true"), { timeout: 8_000 });
    await user.click(screen.getByRole("button", { name: "Write 1 row" }));
    await screen.findByText(/1 row written, 1 refused\./);
    expect(calls.some((c) => c.url.pathname === "/api/v1/projects/helsinki/changes/chg-1")).toBe(true);
  }, 15_000);

  it("asks for a type name when no type fits, and says why the proposal was refused", async () => {
    const calls = server({ op: () => Response.json({ detail: "no right to propose" }, { status: 403 }) });
    const user = userEvent.setup();
    page("notes");
    await user.upload(screen.getByLabelText("Choose a CSV file or an Excel workbook (.xlsx)"), new File(["foo,bar\n1,2\n"], "notes.csv"));
    await screen.findByText(/None of the Smart Data Models types fits/);
    const propose = screen.getByRole("button", { name: "Propose the change" });
    await user.clear(screen.getByLabelText("Type name"));
    await user.type(screen.getByLabelText("Type name"), "lowercase");
    expect(propose).toHaveAttribute("aria-disabled", "true");
    await user.clear(screen.getByLabelText("Type name"));
    await user.type(screen.getByLabelText("Type name"), "Note");
    await user.click(propose);
    expect(await screen.findByRole("alert")).toHaveTextContent("no right to propose");
    expect(calls.some((c) => c.url.pathname === "/api/v1/tools/import-sdm")).toBe(false);
    const op = calls.find((c) => c.url.pathname.endsWith("/ops/jc_space_complete") && (c.body as { propose?: boolean }).propose)?.body as { files: unknown[] };
    expect(op.files).toHaveLength(1);
  });

  it("says it is reading the file while the catalogue answers, and names the draft a refusal blocks on (T-5109)", async () => {
    let answer: (response: Response) => void = () => undefined;
    const detail = "The Endpoint draft 'parking-all' has not passed its check; open it in Drafts, fix it, then propose again.";
    server({
      catalogue: () => new Promise<Response>((resolve) => (answer = resolve)),
      op: () => Response.json({ error: "verdict_required", draft: { kind: "Endpoint", name: "parking-all" }, detail }, { status: 409 }),
    });
    const user = userEvent.setup();
    page("parking");
    await user.upload(screen.getByLabelText("Choose a CSV file or an Excel workbook (.xlsx)"), new File([CSV], "garages.csv", { type: "text/csv" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Reading garages.csv");
    answer(Response.json(CATALOGUE));
    await screen.findByRole("radio", { name: /OffStreetParking/ });
    expect(screen.queryByText(/Reading garages\.csv/)).toBeNull();
    await user.click(screen.getByRole("button", { name: "Propose the change" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(detail);
  });

  it("refuses an empty file before ranking anything", async () => {
    const calls = server();
    const user = userEvent.setup();
    page();
    await user.upload(screen.getByLabelText("Choose a CSV file or an Excel workbook (.xlsx)"), new File(["name,lat\n"], "empty.csv"));
    expect(await screen.findByRole("alert")).toHaveTextContent("no rows");
    expect(calls.some((c) => c.url.pathname === "/api/v1/tools/sdm-catalog")).toBe(false);
  });
});
