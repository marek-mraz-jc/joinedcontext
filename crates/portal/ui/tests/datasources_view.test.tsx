import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { expectOpen } from "./checks";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { endpointOf, endpointView, toEnvelope } from "../src/pages/datasources/DataSourcesPage";
import { rememberPrefill } from "../src/assistant/state";
import type { Manifest } from "../src/api/manifest";
import { knownSecretNames } from "../src/api/manifest";
import { digestOf } from "../src/api/digest";
import { findFormPage } from "./formPage";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const SOURCES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: {
        name: "mqtt-mesto",
        namespace: "banskabystrica",
        title: { sk: "Mestský MQTT broker", en: "City MQTT broker" },
      },
      spec: {
        type: "mqtt",
        mqtt: {
          urls: ["tls://mqtt.banskabystrica.sk:8883"],
          topics: ["sensors/aq/+/reading"],
          username: "bb-collector",
          passwordRef: { name: "mqtt-mesto", key: "password" },
        },
      },
      status: { phase: "Live" },
    },
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: { name: "mhd-vehicles", namespace: "banskabystrica" },
      spec: {
        type: "gtfs-rt",
        gtfsRt: { url: "https://gtfs.banskabystrica.sk/vp.pb", feed: "vehiclePositions" },
      },
    },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-7f3e", namespace: "banskabystrica" },
  status: { lane: "green", phase: "PendingApproval", plan: { create: 1 } },
};

const DRY_RUN = {
  valid: true,
  lane: "green",
  plan: {
    fields: [{ path: "spec.mqtt.urls", to: ["tls://mqtt.banskabystrica.sk:8883"] }],
  },
  probe: { records: 1, bytes: 512, sample: { last_updated: 1789314850, data: { bikes: [] } } },
};

function renderDataSources(drafts: Record<string, unknown> = {}) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = new URL(request.url);
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );

    if (url.pathname.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    const drafted = drafts[url.pathname];
    if (request.method === "GET" && drafted) {
      return json(drafted);
    }
    if (request.method === "POST" || request.method === "PUT") {
      if (url.searchParams.get("dryRun") !== "All") {
        return json(CHANGE, 202);
      }
      // The check answers a verdict fresh for the manifest it judged, as the server does; under
      // strict validation the form proposes nothing without it (PF-57, T-0779).
      return request
        .clone()
        .json()
        .then((body: Record<string, unknown>) => {
          const manifest = { ...body };
          delete manifest.draft;
          return json({
            ...DRY_RUN,
            verdict: {
              ok: true,
              findings: [],
              checkedAt: new Date().toISOString(),
              inputDigest: digestOf(manifest),
            },
          });
        });
    }
    if (url.pathname.endsWith("/datasources")) {
      return json(SOURCES);
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

function writes(fetchMock: ReturnType<typeof vi.fn>): Request[] {
  return fetchMock.mock.calls
    .map((call) => call[0] as Request)
    .filter((request) => (request.method === "POST" || request.method === "PUT") && !String((request as Request).url ?? request).includes("/drafts"));
}

/** The writes that are not the check's dry run: what the person actually proposed. */
function proposals(fetchMock: ReturnType<typeof vi.fn>): Request[] {
  return writes(fetchMock).filter((request) => !new URL(request.url).searchParams.has("dryRun"));
}

describe("data sources view", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/datasources");
  });

  afterEach(() => {
    vi.restoreAllMocks();
    // The runner form cases below render on the page this block leaves open, without a query.
    window.history.pushState({}, "", "/projects/banskabystrica/datasources");
  });

  it("starts a pipeline on a source from its row, the editor opening with that source picked", async () => {
    renderDataSources();
    const row = (await screen.findByRole("link", { name: "City MQTT broker" })).closest("tr") as HTMLElement;
    await userEvent.click(within(row).getByRole("button", { name: /More actions/ }));
    await userEvent.click(await screen.findByRole("menuitem", { name: en.datasources.newPipeline }));
    await waitFor(() => expect(window.location.pathname).toBe("/projects/banskabystrica/pipelines"));
    const page = await findFormPage();
    await waitFor(() =>
      expect(within(page).getAllByRole("combobox").some((select) => (select as HTMLSelectElement).value.includes(SOURCES.items[0].metadata.name))).toBe(true),
    );
  });

  it("lists every source with its type, endpoint and the names of its credentials", async () => {
    renderDataSources();

    // The name is in two columns, its own and the credentials, so the cells are read in order.
    const row = (await screen.findByText("City MQTT broker")).closest("tr") as HTMLElement;
    const cells = within(row).getAllByRole("cell");
    expect(cells[0]).toHaveTextContent("mqtt-mesto");
    // The title is the link and reads first; the id is the line under it.
    expect(within(cells[0]).getByRole("link")).toHaveTextContent("City MQTT broker");
    expect(cells[1]).toHaveTextContent(en.datasources.type.mqtt);
    expect(cells[2]).toHaveTextContent("mqtt.banskabystrica.sk:8883");
    expect(cells[3]).toHaveTextContent("mqtt-mesto");

    const feed = (await screen.findByText("mhd-vehicles")).closest("tr") as HTMLElement;
    expect(within(feed).getByText(en.datasources.type["gtfs-rt"])).toBeInTheDocument();
    expect(within(feed).getByText(en.datasources.noSecret)).toBeInTheDocument();
  });

  // T-3211: a person adding their first source most often has the address of a file or an API,
  // so a new source opens on HTTP and no other connection's fields.
  it("names the secret an address template reads in the list, never the template (T-4991)", async () => {
    const templated = {
      ...SOURCES,
      items: [{ ...SOURCES.items[1], spec: { type: "nats", input: { urls: ["nats://demo:${DEMO_FEED_PASSWORD}@nats.example:4222"] } } }],
    };
    renderDataSources({ "/api/v1/projects/banskabystrica/datasources": templated });
    expect(await screen.findByText(en.datasources.secretFrom.replace("{name}", "DEMO_FEED_PASSWORD"))).toBeInTheDocument();
    expect(screen.getByText("nats.example:4222")).toBeInTheDocument();
    expect(document.body.textContent).not.toContain("${DEMO_FEED_PASSWORD}");
  });

  it("opens a new source on the HTTP connection and no other", async () => {
    renderDataSources();

    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();

    expect(within(dialog).getByLabelText(en.datasources.field.type)).toHaveValue("http");
    expect(within(dialog).getByLabelText(/Method/)).toBeInTheDocument();
    expect(within(dialog).getByLabelText(/Timeout/)).toBeInTheDocument();
    expect(within(dialog).queryByLabelText(/Broker URLs/)).not.toBeInTheDocument();
  });

  it("swaps the form to the MQTT connection, and no other, when the type selector changes", async () => {
    renderDataSources();

    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.selectOptions(within(dialog).getByLabelText(en.datasources.field.type), "mqtt");

    expect(within(dialog).getByLabelText(/Broker URLs/)).toBeInTheDocument();
    expect(within(dialog).getByLabelText(/User name/)).toBeInTheDocument();
    expect(within(dialog).queryByLabelText(/Opening message/)).not.toBeInTheDocument();
    expect(within(dialog).queryByLabelText(/Feed/)).not.toBeInTheDocument();
    expect(within(dialog).queryByLabelText(/Method/)).not.toBeInTheDocument();
  });

  // T-3211: QoS names what each level guarantees; nobody types a number they have to look up.
  it("offers the MQTT delivery levels by what they guarantee", async () => {
    renderDataSources();

    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.selectOptions(within(dialog).getByLabelText(en.datasources.field.type), "mqtt");

    const qos = within(dialog).getByLabelText(/QoS/);
    const labels = [...qos.querySelectorAll("option")].map((option) => option.textContent);
    expect(labels).toEqual(expect.arrayContaining([en.choice.mqttQos["0"], en.choice.mqttQos["1"], en.choice.mqttQos["2"]]));
  });

  // T-2758: the type decides every field of the form, so it is the form's first question, not a
  // select on the list page above the button that opens it.
  it("asks for the type first, inside the form, and keeps the name when it changes", async () => {
    renderDataSources();

    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    const type = within(dialog).getByLabelText(en.datasources.field.type);
    const name = within(dialog).getByLabelText(/Name/);
    expect(type.compareDocumentPosition(name) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

    await userEvent.type(name, "free-bikes");
    await userEvent.selectOptions(type, "mqtt");
    expect(within(dialog).getByLabelText(/Broker URLs/)).toBeInTheDocument();
    expect(within(dialog).getByLabelText(/Name/)).toHaveValue("free-bikes");
  });

  it("writes the clean session sentence once, under its checkbox", async () => {
    renderDataSources();
    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.selectOptions(within(dialog).getByLabelText(en.datasources.field.type), "mqtt");
    const box = within(dialog).getByRole("checkbox", { name: /Clean session/ });
    const sentences = within(dialog).getAllByText(/start a fresh session on each connection/);
    expect(sentences).toHaveLength(1);
    expect(box.compareDocumentPosition(sentences[0]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("puts no type select on the list page", async () => {
    renderDataSources();
    await screen.findByRole("button", { name: en.datasources.add });
    expect(screen.queryByLabelText(en.datasources.field.type)).toBeNull();
  });

  /** The value of a credential is never a field; only the reference to it is (CC-06, MF-35). */
  it("asks for the name and the key of a secret, never for the secret", async () => {
    renderDataSources();

    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.selectOptions(within(dialog).getByLabelText(en.datasources.field.type), "mqtt");

    expect(within(dialog).getByText(en.datasources.field.password)).toBeInTheDocument();
    expect(within(dialog).getByText(en.datasources.secretHint)).toBeInTheDocument();
    const password = within(dialog).queryByLabelText(/^Password$/);
    expect(password).toBeNull();

    // The picker offers the names the project already uses, and nothing from the store itself.
    const name = within(dialog).getAllByLabelText(/Secret name/)[0];
    const list = name.getAttribute("list");
    expect(list).toBeTruthy();
    const options = document.getElementById(list as string)?.querySelectorAll("option") ?? [];
    expect([...options].map((option) => option.getAttribute("value"))).toContain("mqtt-mesto");
  });

  it("refuses a name that is not a lowercase slug before anything is sent", async () => {
    const fetchMock = renderDataSources();

    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.type(within(dialog).getByLabelText(/Name/), "MQTT Mesta");
    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.propose }));

    const alerts = await within(dialog).findAllByRole("alert");
    // What a name may be, not the pattern it broke (T-0960).
    expect(alerts.some((alert) => alert.textContent?.includes(en.form.dns1123))).toBe(true);
    expect(writes(fetchMock)).toHaveLength(0);
  });

  it("proposes a change carrying the manifest the form describes", async () => {
    const fetchMock = renderDataSources();

    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.selectOptions(within(dialog).getByLabelText(en.datasources.field.type), "websocket");
    await userEvent.type(within(dialog).getByLabelText(/Name/), "aq-stream");
    await userEvent.type(
      within(dialog).getByLabelText(/URL/),
      "wss://feed.banskabystrica.sk/aq",
    );
    // Strict validation proposes nothing without a fresh green verdict (T-0779).
    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.check }));
    await waitFor(() =>
      expectOpen(within(dialog).getByRole("button", { name: en.datasources.propose })),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.propose }));

    await waitFor(() => expect(proposals(fetchMock)).toHaveLength(1));
    const request = proposals(fetchMock)[0];
    expect(request.method).toBe("POST");
    expect(new URL(request.url).pathname).toBe("/api/v1/projects/banskabystrica/datasources");
    await expect(request.clone().json()).resolves.toMatchObject({
      kind: "DataSource",
      metadata: { name: "aq-stream", namespace: "banskabystrica" },
      spec: { type: "websocket", webSocket: { url: "wss://feed.banskabystrica.sk/aq" } },
    });

    expect(await screen.findByText("chg-7f3e")).toBeInTheDocument();
  });

  it("names no authorization for an HTTP source with no credential (T-0626)", async () => {
    const fetchMock = renderDataSources();

    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.selectOptions(within(dialog).getByLabelText(en.datasources.field.type), "http");
    await userEvent.type(within(dialog).getByLabelText(/Name/), "free-bikes");
    await userEvent.type(within(dialog).getByLabelText(/URL/), "https://gbfs.example.org/free_bike_status.json");
    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.check }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const body = (await writes(fetchMock)[0].clone().json()) as { spec: { http: Record<string, unknown> } };
    expect(body.spec.http.url).toBe("https://gbfs.example.org/free_bike_status.json");
    expect(body.spec.http).not.toHaveProperty("authorization");
  });

  /** MF-13: the plan is a dry run against the same route, not a second endpoint. */
  it("shows what one fetch of the feed returned beside the plan (MF-39)", async () => {
    renderDataSources();
    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.selectOptions(within(dialog).getByLabelText(en.datasources.field.type), "http");
    await userEvent.type(within(dialog).getByLabelText(/Name/), "free-bikes");
    await userEvent.type(within(dialog).getByLabelText(/URL/), "https://gbfs.example.org/free_bike_status.json");
    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.check }));
    const probe = await within(dialog).findByTestId("datasource-probe");
    expect(probe).toHaveTextContent("1 records, 512 bytes");
    // T-5516: the sample reads as rows, never as JSON text.
    expect(within(probe).getByText("bikes")).toBeInTheDocument();
    expect(probe.textContent).not.toContain("{\n");
    // A change to the form is a new manifest: the probe belongs to the old one.
    await userEvent.type(within(dialog).getByLabelText(/Timeout/), "15s");
    expect(within(dialog).queryByTestId("datasource-probe")).toBeNull();
  });

  it("fills the form from a template and checks it at once, with the template's type (T-3249)", async () => {
    const fetchMock = renderDataSources();
    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    const gallery = within(dialog).getByTestId("datasource-templates");
    // T-6163: closed at first, one summary line above the first field.
    expect(gallery).not.toHaveAttribute("open");
    await userEvent.click(within(gallery).getByText(en.datasources.template.title));
    await userEvent.click(within(gallery).getByRole("button", { name: new RegExp(en.datasources.template["gtfs-rt"].title) }));
    await userEvent.type(within(gallery).getByLabelText(new RegExp(en.datasources.template["gtfs-rt"].url)), "https://gtfs.example.org/vehicle-positions.pb");
    expect(within(gallery).getByLabelText(/Name/)).toHaveValue("vehicle-positions");
    await userEvent.click(within(gallery).getByRole("button", { name: en.datasources.template.use }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const dry = writes(fetchMock)[0];
    expect(new URL(dry.url).searchParams.get("dryRun")).toBe("All");
    const body = (await dry.clone().json()) as { metadata: { name: string }; spec: Record<string, unknown> };
    expect(body.metadata.name).toBe("vehicle-positions");
    expect(body.spec).toEqual({ type: "gtfs-rt", gtfsRt: { url: "https://gtfs.example.org/vehicle-positions.pb", feed: "vehiclePositions" } });
    expect(await within(dialog).findByTestId("datasource-probe")).toHaveTextContent("1 records, 512 bytes");
    expect(screen.getByLabelText(en.datasources.field.type)).toHaveValue("gtfs-rt");
  });

  it("refuses a CKAN dataset page at the field and sends nothing (T-3249)", async () => {
    const fetchMock = renderDataSources();
    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const gallery = within(await findFormPage()).getByTestId("datasource-templates");
    await userEvent.click(within(gallery).getByRole("button", { name: new RegExp(en.datasources.template.ckan.title) }));
    const address = within(gallery).getByLabelText(new RegExp(en.datasources.template.ckan.url));
    await userEvent.type(address, "https://data.gov.sk/dataset/kvalita-ovzdusia");
    await userEvent.click(within(gallery).getByRole("button", { name: en.datasources.template.use }));
    expect(within(gallery).getByText(en.datasources.template.problem.ckanDataset)).toBeInTheDocument();
    expect(address).toHaveAttribute("aria-invalid", "true");
    expect(writes(fetchMock)).toHaveLength(0);
  });

  it("opens the dialog filled when the assistant sent a data source ahead (AG-61)", async () => {
    rememberPrefill("/projects/banskabystrica/datasources", {
      type: "http",
      name: "hsl-citybikes-free",
      http: { url: "https://gbfs.example.org/free_bike_status.json", timeout: "15s" },
    });
    renderDataSources();
    const dialog = await findFormPage();
    expect(within(dialog).getByLabelText(/Name/)).toHaveValue("hsl-citybikes-free");
    expect(within(dialog).getByLabelText(/URL/)).toHaveValue("https://gbfs.example.org/free_bike_status.json");
    expect(screen.getByLabelText(en.datasources.field.type)).toHaveValue("http");
  });

  it("shows the planned change before the proposal is sent", async () => {
    const fetchMock = renderDataSources();

    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.selectOptions(within(dialog).getByLabelText(en.datasources.field.type), "mqtt");
    await userEvent.type(within(dialog).getByLabelText(/Name/), "mqtt-novy");
    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.check }));

    expect(await within(dialog).findByTitle("spec.mqtt.urls")).toBeInTheDocument();
    const dry = writes(fetchMock);
    expect(dry).toHaveLength(1);
    expect(new URL(dry[0].url).searchParams.get("dryRun")).toBe("All");
  });

  it("edits an existing source in place, at its own path and its own type", async () => {
    const fetchMock = renderDataSources();

    const row = (await screen.findByText("City MQTT broker")).closest("tr") as HTMLElement;
    // Edit lives in the row's menu now (T-2287).
    await userEvent.click(within(row).getByRole("button", { name: /More actions/ }));
    await userEvent.click(await screen.findByRole("menuitem", { name: en.resourceEdit.button }));
    const dialog = await findFormPage();

    expect(within(dialog).getByLabelText(/Name/)).toHaveValue("mqtt-mesto");
    expect(within(dialog).getByLabelText(/User name/)).toHaveValue("bb-collector");
    await userEvent.clear(within(dialog).getByLabelText(/User name/));
    await userEvent.type(within(dialog).getByLabelText(/User name/), "bb-reader");
    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.check }));
    await waitFor(() =>
      expectOpen(within(dialog).getByRole("button", { name: en.datasources.propose })),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.propose }));

    await waitFor(() => expect(proposals(fetchMock)).toHaveLength(1));
    const request = proposals(fetchMock)[0];
    expect(request.method).toBe("PUT");
    expect(new URL(request.url).pathname).toBe(
      "/api/v1/projects/banskabystrica/datasources/mqtt-mesto",
    );
    await expect(request.clone().json()).resolves.toMatchObject({
      spec: { type: "mqtt", mqtt: { username: "bb-reader" } },
    });
  });

  it("opens the draft the address names in its own type's form, not the page's first type", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/datasources?draft=aq-opendata");
    const manifest = toEnvelope("banskabystrica", "http", {
      name: "aq-opendata",
      http: { url: "https://opendata.banskabystrica.sk/aq.json", verb: "GET" },
    });
    renderDataSources({
      "/api/v1/projects/banskabystrica/drafts/DataSource/aq-opendata": {
        project: "banskabystrica",
        kind: "DataSource",
        name: "aq-opendata",
        manifest,
        touchedBy: "jana.kovacova",
        touchedKind: "person",
        version: 2,
        updatedAt: "2026-09-15T05:00:00Z",
      },
    });

    const dialog = await findFormPage();
    expect(await within(dialog).findByLabelText(/^URL/)).toHaveValue("https://opendata.banskabystrica.sk/aq.json");
    expect(within(dialog).queryByLabelText(/Broker URLs/)).not.toBeInTheDocument();
  });

  it("opens a source's editor on the change the assistant made, and sends nothing until proposed (AG-77)", async () => {
    const source = SOURCES.items[0];
    const changed = { ...source, status: undefined, spec: { ...source.spec, mqtt: { ...source.spec.mqtt, username: "bb-reader" } } };
    rememberPrefill("/projects/banskabystrica/datasources?edit=mqtt-mesto", changed);
    window.history.pushState({}, "", "/projects/banskabystrica/datasources?edit=mqtt-mesto&draft=mqtt-mesto");
    const fetchMock = renderDataSources();

    const dialog = await findFormPage();
    expect(within(dialog).getByLabelText(/Name/)).toHaveValue("mqtt-mesto");
    expect(within(dialog).getByLabelText(/User name/)).toHaveValue("bb-reader");
    expect(proposals(fetchMock)).toHaveLength(0);

    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.check }));
    await waitFor(() =>
      expectOpen(within(dialog).getByRole("button", { name: en.datasources.propose })),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.propose }));
    await waitFor(() => expect(proposals(fetchMock)).toHaveLength(1));
    expect(proposals(fetchMock)[0].method).toBe("PUT");
    expect(new URL(proposals(fetchMock)[0].url).pathname).toBe("/api/v1/projects/banskabystrica/datasources/mqtt-mesto");
  });
});

describe("the manifest a data source form describes", () => {
  // A form is an address of its own (T-2474): each case starts on the list, not on the form the
  // case before it left open.
  beforeEach(() => {
    window.history.pushState({}, "", "/projects/banskabystrica/datasources");
  });

  it("puts the metadata in metadata and everything else under the declared type", () => {
    const envelope = toEnvelope("banskabystrica", "http", {
      name: "aq-opendata",
      title: "Open data",
      http: { url: "https://opendata.banskabystrica.sk/aq.json", verb: "GET" },
      // rjsf leaves this behind for a group the user opened and left alone.
      tls: { caCertRef: {} },
    });

    expect(envelope).toEqual({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: {
        name: "aq-opendata",
        namespace: "banskabystrica",
        title: "Open data",
      },
      spec: {
        type: "http",
        http: { url: "https://opendata.banskabystrica.sk/aq.json", verb: "GET" },
      },
    });
  });

  it("reads the endpoint and the credential names off a stored manifest", () => {
    const [mqtt, gtfs] = SOURCES.items;
    expect(endpointOf(mqtt.spec)).toBe("tls://mqtt.banskabystrica.sk:8883");
    expect(endpointOf(gtfs.spec)).toBe("https://gtfs.banskabystrica.sk/vp.pb");
    expect(knownSecretNames(SOURCES.items as unknown as Manifest[])).toEqual(["mqtt-mesto"]);
  });

  it("shows an address as host and path, the query on hover and no credential (T-4991)", () => {
    const nats = endpointView("nats://demo:${DEMO_FEED_PASSWORD}@demo-nats.dev.svc.cluster.local:4222");
    expect(nats).toEqual({ shown: "demo-nats.dev.svc.cluster.local:4222", full: "nats://demo-nats.dev.svc.cluster.local:4222", secrets: ["DEMO_FEED_PASSWORD"] });
    expect(JSON.stringify(nats)).not.toContain("${");
    const wfs = endpointView("https://kartta.hel.fi/ws/geoserver/avoindata/wfs?service=WFS&typeNames=avoindata:Kaupunginosajako");
    expect(wfs.shown).toBe("kartta.hel.fi/ws/geoserver/avoindata/wfs");
    expect(wfs.full).toContain("?service=WFS");
    // A literal password in the address is dropped, not shown.
    expect(endpointView("postgres://reader:hunter2@db.example.org/city")).toEqual({ shown: "db.example.org/city", full: "postgres://db.example.org/city", secrets: [] });
    // A raw @, / or ? inside the password leaves no part of it in the cell or the title (T-4991 review).
    for (const leaky of ["https://user:p@ss@host.example/x", "https://user:pa/ss@host.example/x", "https://user:pa?ss@host.example/x", "postgres://reader:hunter@2@host.example/x"]) {
      const view = endpointView(leaky);
      expect(view.shown).toBe("host.example/x");
      expect(view.full).toBe(`${leaky.split("://")[0]}://host.example/x`);
    }
    expect(endpointView("https://user:p/a@ss@host.example/x").shown).toBe("host.example/x");
    // An @ only in the path keeps the host.
    expect(endpointView("https://host.example/users/@me").shown).toBe("host.example/users/@me");
    expect(endpointView("http://[::1]:8080/@me").shown).toBe("[::1]:8080/@me");
    expect(endpointView("wss://mqtt.hsl.fi:443/").shown).toBe("mqtt.hsl.fi:443");
    expect(endpointView("/data/in.csv, /data/b.csv").shown).toBe("/data/in.csv, /data/b.csv");
    expect(endpointView("https://x.example/${TOKEN}/feed").shown).toBe("x.example/…/feed");
  });

  it("endpointOf returns addresses, paths, dsn, url or type name for runner inputs", () => {
    expect(
      endpointOf({
        type: "kafka",
        input: { addresses: ["kafka1:9092", "kafka2:9092"] },
      }),
    ).toBe("kafka1:9092, kafka2:9092");

    expect(
      endpointOf({
        type: "csv",
        input: { paths: ["/data/records1.csv", "/data/records2.csv"] },
      }),
    ).toBe("/data/records1.csv, /data/records2.csv");

    expect(
      endpointOf({
        type: "sql_select",
        input: { dsn: "postgres://user:pass@host:5432/db" },
      }),
    ).toBe("postgres://user:pass@host:5432/db");

    expect(
      endpointOf({
        type: "generate",
        input: { mapping: "root = {}" },
      }),
    ).toBe("generate");
  });

  /** A secret field of a runner input is a reference picker, not a crash (PL-50, MF-35). */
  it("renders a runner input whose fields include a secret as a reference picker", async () => {
    renderDataSources();

    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.selectOptions(within(dialog).getByLabelText(en.datasources.field.type), "nats");

    expect((await within(dialog).findAllByPlaceholderText(/Secret name/)).length).toBeGreaterThan(0);
  });

  it("describes a list field once, under its own heading", async () => {
    renderDataSources();

    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.selectOptions(within(dialog).getByLabelText(en.datasources.field.type), "csv");

    expect(await within(dialog).findAllByText(/^A list of file paths to read from/)).toHaveLength(1);
  });

  it("renders grouped select with an optgroup 'Message brokers' containing 'kafka'", async () => {
    renderDataSources();

    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    const typeSelect = within(dialog).getByLabelText(en.datasources.field.type) as HTMLSelectElement;
    expect(typeSelect).toBeInTheDocument();

    await waitFor(() => {
      const brokerGroup = typeSelect.querySelector('optgroup[label="Message brokers"]');
      expect(brokerGroup).not.toBeNull();
      const kafkaOption = brokerGroup?.querySelector('option[value="kafka"]');
      expect(kafkaOption).not.toBeNull();
      expect(kafkaOption?.textContent).toBe("Kafka");
    });
  });

  it("drafts a runner input's map as the map it is, as the proposal will carry it", async () => {
    const fetchMock = renderDataSources();
    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.selectOptions(within(dialog).getByLabelText(en.datasources.field.type), "http_client");
    // The data source's own name comes first; the runner's negotiate › user › name is labelled
    // "Name" too since its fields read as words (T-4165).
    await userEvent.type(within(dialog).getAllByLabelText(/^Name/)[0], "feed-csv");
    const headers = await waitFor(() => {
      const box = dialog.querySelector<HTMLTextAreaElement>("#root_headers");
      expect(box).not.toBeNull();
      return box as HTMLTextAreaElement;
    });
    await userEvent.type(headers, "Accept: text/csv");
    // The draft is written after a pause in typing; it is what the proposal takes (T-0769).
    const drafted = async () => {
      const puts = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .filter((request) => request.method === "PUT" && request.url.includes("/drafts/"));
      return puts.length > 0 ? ((await puts[puts.length - 1].clone().json()) as { manifest?: { spec?: { input?: Record<string, unknown> } } }) : undefined;
    };
    await waitFor(async () => expect((await drafted())?.manifest?.spec?.input?.headers).toEqual({ Accept: "text/csv" }), { timeout: 4000 });
  });

  it("shows an error and does not call the API when a runner form has invalid YAML", async () => {
    const fetchMock = renderDataSources();

    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    await findFormPage();

    // Select a runner input with a YAML-parsed field by simulating a YAML error in form data
    const invalidForm = {
      name: "custom-input",
      type: "kafka",
      addresses: ["kafka:9092"],
      tls: "{ enabled: [true",
    };

    expect(() => toEnvelope("banskabystrica", "kafka", invalidForm)).toThrow(/^tls: not valid YAML/);
    expect(writes(fetchMock)).toHaveLength(0);
  });
});
