import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { GrafanaPage, SavedVisualization, VisualizationForm, VisualizationPreview } from "../src/pages/grafana/GrafanaPage";
import "../src/i18n";

const options = {
  endpoints: [
    { name: "air", title: "Air quality", slug: "airslug", public: true, usable: true },
    { name: "ops", title: "Bike operations", slug: "opsslug", public: false, usable: false },
  ],
  types: ["AirQualityObserved"],
  attributes: [
    { name: "pm25", kinds: ["timeseries", "stat", "table"] },
    { name: "location", kinds: ["table", "map"] },
  ],
};

type Answer = { status?: number; body: unknown };
let answers: Record<string, Answer>;
let sent: { method: string; url: string; body?: unknown }[];

beforeEach(() => {
  sent = [];
  answers = {
    "GET /api/v1/projects/helsinki/grafana": {
      body: { ready: true, mayCreate: true, items: [] },
    },
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string) => {
      const request = input instanceof Request ? input : new Request(new URL(input, "http://portal"));
      const url = new URL(request.url);
      const text = request.method === "GET" ? "" : await request.clone().text();
      sent.push({ method: request.method, url: url.pathname + url.search, body: text ? JSON.parse(text) : undefined });
      const key = `${request.method} ${url.pathname}`;
      const rows: Answer | undefined = url.pathname.startsWith("/api/endpoint/") ? { body: [{ id: "urn:ngsi-ld:A:one", name: "Kallio", pm25: 4.2 }] } : undefined;
      const answer = rows ?? answers[key] ?? (key.endsWith("/grafana/options") ? { body: options } : undefined);
      if (!answer) return new Response("{}", { status: 404 });
      if (answer.status === 204) return new Response(null, { status: 204 });
      return new Response(JSON.stringify(answer.body), {
        status: answer.status ?? 200,
        headers: { "Content-Type": "application/json" },
      });
    }),
  );
});

afterEach(() => vi.unstubAllGlobals());

function renderInClient(node: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
}

const item = {
  id: "ab12",
  title: "PM2.5",
  endpoint: "air",
  endpointTitle: "Air quality",
  type: "AirQualityObserved",
  attribute: "pm25",
  kind: "timeseries" as const,
  createdAt: "2026-11-02T00:00:00Z",
  openUrl: "https://dev.example/grafana/d/pv-ab12?orgId=2",
};

async function fill() {
  const user = userEvent.setup();
  await screen.findByRole("option", { name: "Air quality" });
  await user.selectOptions(screen.getByLabelText("Endpoint"), "air");
  await waitFor(() => expect(screen.getByLabelText("Type")).toBeEnabled());
  await user.selectOptions(screen.getByLabelText("Type"), "AirQualityObserved");
  await waitFor(() => expect(screen.getByLabelText("Attribute")).toBeEnabled());
  return user;
}

describe("the Grafana page", () => {
  it("says why when Grafana is not ready and offers no form", async () => {
    answers["GET /api/v1/projects/helsinki/grafana"] = {
      body: { ready: false, reason: "Grafana is not set up", mayCreate: true, items: [] },
    };
    renderInClient(<GrafanaPage project="helsinki" />);
    expect(await screen.findByText("Grafana is not set up")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /save to grafana/i })).toBeNull();
  });

  it("shows a reader what is saved, without the form or a remove button", async () => {
    answers["GET /api/v1/projects/helsinki/grafana"] = {
      body: { ready: true, mayCreate: false, items: [item] },
    };
    renderInClient(<GrafanaPage project="helsinki" />);
    expect(await screen.findByRole("link", { name: "Open PM2.5 in Grafana" })).toHaveAttribute("href", item.openUrl);
    expect(screen.queryByLabelText("Endpoint")).toBeNull();
    expect(screen.queryByRole("button", { name: "Remove PM2.5" })).toBeNull();
  });

  it("tells what to do next on an empty list", async () => {
    renderInClient(<GrafanaPage project="helsinki" />);
    expect(await screen.findByText("Nothing saved yet")).toBeInTheDocument();
  });
});

describe("the form", () => {
  it("offers what the server offers, names what Grafana cannot read, and previews the choice", async () => {
    renderInClient(<VisualizationForm project="helsinki" />);
    await fill();
    expect(screen.getByRole("option", { name: "Air quality" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Bike operations" })).toBeNull();
    expect(screen.getByText(/Not in Grafana yet: Bike operations/)).toBeInTheDocument();
    await userEvent.selectOptions(screen.getByLabelText("Attribute"), "pm25");
    expect(await screen.findByText("Kallio")).toBeInTheDocument();
    expect(screen.getByText("4.2")).toBeInTheDocument();
    expect(sent.find((call) => call.url.startsWith("/api/endpoint/airslug/"))?.url).toContain("attrs=name%2Cpm25");
  });

  it("keeps the kind to what the attribute can draw", async () => {
    renderInClient(<VisualizationForm project="helsinki" />);
    const user = await fill();
    await user.selectOptions(screen.getByLabelText("Attribute"), "location");
    expect(screen.getByLabelText("Visualization")).toHaveValue("table");
    expect(screen.queryByRole("option", { name: "Time series" })).toBeNull();
    expect(screen.getByRole("option", { name: "Map" })).toBeInTheDocument();
  });

  it("saves the choices and gives the link to open them", async () => {
    answers["POST /api/v1/projects/helsinki/grafana"] = { status: 201, body: item };
    renderInClient(<VisualizationForm project="helsinki" />);
    const user = await fill();
    await user.selectOptions(screen.getByLabelText("Attribute"), "pm25");
    expect(screen.getByRole("button", { name: /save to grafana/i })).toHaveAttribute("aria-disabled", "true");
    await user.type(screen.getByLabelText("Name"), "PM2.5");
    await user.click(screen.getByRole("button", { name: /save to grafana/i }));
    expect(await screen.findByRole("link", { name: "Open it" })).toHaveAttribute("href", item.openUrl);
    expect(sent.find((call) => call.method === "POST")?.body).toEqual({
      title: "PM2.5",
      endpoint: "air",
      type: "AirQualityObserved",
      attribute: "pm25",
      kind: "timeseries",
    });
  });

  it("shows the server's refusal beside the form", async () => {
    answers["POST /api/v1/projects/helsinki/grafana"] = {
      status: 422,
      body: { status: 422, title: "Unprocessable", detail: "a stat cannot draw name; choose another kind" },
    };
    renderInClient(<VisualizationForm project="helsinki" />);
    const user = await fill();
    await user.selectOptions(screen.getByLabelText("Attribute"), "pm25");
    await user.type(screen.getByLabelText("Name"), "x");
    await user.click(screen.getByRole("button", { name: /save to grafana/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent("choose another kind");
  });
});

describe("the preview", () => {
  it("says so when a person may not read the rows or there are none", async () => {
    const refused = new Response(JSON.stringify({ status: 403, title: "Forbidden", detail: "not yours" }), {
      status: 403,
      headers: { "Content-Type": "application/json" },
    });
    vi.mocked(fetch).mockResolvedValueOnce(refused);
    const first = renderInClient(<VisualizationPreview slug="a" type="T" attribute="x" />);
    expect(await screen.findByText("not yours")).toBeInTheDocument();
    first.unmount();
    vi.mocked(fetch).mockResolvedValueOnce(new Response("[]", { status: 200 }));
    renderInClient(<VisualizationPreview slug="b" type="T" attribute="x" />);
    expect(await screen.findByText(/holds no rows/)).toBeInTheDocument();
  });

  it("shows a place by its coordinates, never as raw JSON", async () => {
    const rows = [{ id: "urn:x:7", location: { type: "Point", coordinates: [24.9, 60.1] } }];
    vi.mocked(fetch).mockResolvedValueOnce(new Response(JSON.stringify(rows), { status: 200 }));
    renderInClient(<VisualizationPreview slug="c" type="T" attribute="location" />);
    expect(await screen.findByText("24.9, 60.1")).toBeInTheDocument();
    expect(screen.getByText("7")).toBeInTheDocument();
  });
});

describe("a saved visualization", () => {
  it("asks once more before it is removed", async () => {
    answers[`DELETE /api/v1/projects/helsinki/grafana/${item.id}`] = { status: 204, body: null };
    renderInClient(
      <ul>
        <SavedVisualization project="helsinki" item={item} mayRemove />
      </ul>,
    );
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Remove PM2.5" }));
    await user.click(screen.getByRole("button", { name: "Keep" }));
    expect(sent.some((call) => call.method === "DELETE")).toBe(false);
    await user.click(screen.getByRole("button", { name: "Remove PM2.5" }));
    await user.click(screen.getByRole("button", { name: "Remove from Grafana" }));
    await waitFor(() => expect(sent.some((call) => call.method === "DELETE")).toBe(true));
  });
});
