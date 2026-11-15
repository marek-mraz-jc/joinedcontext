/**
 * T-3249, T-7094 (G-22): the data source wizard fills an existing type's form from what a person
 * types and picks: an open data API listed and paged by the server, a file split into rows, a
 * credential picked as a secret, never typed.
 */
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { TemplateGallery } from "../src/pages/datasources/TemplateGallery";
import { TEMPLATES, fromTemplate, nameFrom, pagingUrl, pagingWords, problemOf, wfsUrl } from "../src/pages/datasources/templates";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

/** The wizard on a page whose server answers `answer`; what it was asked is kept. */
async function renderWizard(answer: (path: string, body: unknown) => Response | undefined = () => undefined, secrets: string[] = []) {
  const onUse = vi.fn();
  const onPick = vi.fn();
  const asked: Array<{ path: string; body: unknown }> = [];
  const view = renderPage(
    <TemplateGallery project="helsinki" secrets={secrets} others={[{ type: "mqtt", label: "MQTT" }]} onUse={onUse} onPick={onPick} />,
    {
      path: "/projects/helsinki/datasources",
      answer: async (url, request) => {
        if (request.method !== "POST") return undefined;
        const body: unknown = await request.json();
        asked.push({ path: url.pathname, body });
        return answer(url.pathname, body);
      },
    },
  );
  await screen.findByTestId("datasource-templates");
  return { onUse, onPick, asked, view };
}

const template = (id: string) => {
  const found = TEMPLATES.find((one) => one.id === id);
  if (!found) throw new Error(id);
  return found;
};

describe("data source templates", () => {
  it("turns each template into the form of an existing type", () => {
    expect(fromTemplate(template("csv"), { name: "ovzdusie", url: " https://data.example.org/a.csv " })).toEqual({
      type: "http",
      form: { name: "ovzdusie", http: { url: "https://data.example.org/a.csv", verb: "GET" } },
    });
    expect(fromTemplate(template("gtfs-rt"), { name: "dpb", url: "https://gtfs.example.org/vp.pb" })).toEqual({
      type: "gtfs-rt",
      form: { name: "dpb", gtfsRt: { url: "https://gtfs.example.org/vp.pb", feed: "vehiclePositions" } },
    });
    const wfs = fromTemplate(template("wfs"), { name: "parky", url: "https://gis.example.org/wfs", layer: "mesto:parky" });
    expect((wfs.form.http as { url: string }).url).toBe(
      "https://gis.example.org/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=mesto%3Aparky&outputFormat=application%2Fjson",
    );
  });

  it("keeps a WFS address that already asks for features, and replaces a half-written query", () => {
    const ready = "https://gis.example.org/wfs?request=GetFeature&typeNames=a&outputFormat=json";
    expect(wfsUrl(ready, "b")).toBe(ready);
    expect(wfsUrl("https://gis.example.org/wfs?SERVICE=WMS&map=x", "b")).toBe(
      "https://gis.example.org/wfs?map=x&service=WFS&version=2.0.0&request=GetFeature&typeNames=b&outputFormat=application%2Fjson",
    );
  });

  it("refuses what the template cannot read, with the reason", () => {
    const ok = { name: "zdroj", url: "https://data.example.org/a.csv" };
    expect(problemOf(template("csv"), ok)).toBeUndefined();
    expect(problemOf(template("csv"), { ...ok, name: "Zdroj 1" })).toBe("name");
    expect(problemOf(template("csv"), { ...ok, url: "data.example.org/a.csv" })).toBe("url");
    expect(problemOf(template("csv"), { ...ok, url: "ftp://data.example.org/a.csv" })).toBe("url");
    expect(problemOf(template("ckan"), { ...ok, url: "https://data.gov.sk/dataset/ovzdusie-2026" })).toBe("ckanDataset");
    expect(problemOf(template("ckan"), { ...ok, url: "https://data.gov.sk/dataset/x/resource/y/download/a.csv" })).toBeUndefined();
    expect(problemOf(template("wfs"), { ...ok, url: "https://gis.example.org/wfs" })).toBe("layer");
    expect(problemOf(template("wfs"), { ...ok, url: "https://gis.example.org/parky.geojson" })).toBeUndefined();
  });

  it("writes a preset, a picked credential, a CSV's rows and a JSON API's walk as the form's fields (T-7094)", () => {
    const secret = { name: "hsl-api", key: "token" };
    expect(fromTemplate(template("api-arcgisFeature"), { name: "trees", url: "https://gis.example/FeatureServer", secret })).toEqual({
      type: "http",
      form: {
        name: "trees",
        http: {
          url: "https://gis.example/FeatureServer",
          preset: { kind: "arcgisFeature" },
          verb: "GET",
          authorization: { headerRef: secret },
        },
      },
    });
    expect(fromTemplate(template("csv"), { name: "a", url: "https://data.example.org/a.csv", rows: true }).form.http).toEqual({
      url: "https://data.example.org/a.csv",
      verb: "GET",
      format: "csv",
    });
    const paging = { mode: "nextUrl", next: "@odata.nextLink" };
    expect(fromTemplate(template("json"), { name: "a", url: "https://api.example/x", records: "value", paging }).form.http).toEqual({
      url: "https://api.example/x",
      verb: "GET",
      records: "value",
      paging,
    });
    expect(problemOf(template("json"), { name: "a", url: "https://api.example/x", secret: { name: "s", key: " " } })).toBe("secretKey");
  });

  it("reads an API's records resource for its pages and says the guess in words (T-7094)", () => {
    const wfs = template("api-wfs");
    expect(pagingUrl(wfs, { name: "", url: "", http: { url: "https://kartta.hel.fi/wfs", preset: { kind: "wfs", typeNames: "a:b" } } })).toBe(
      wfsUrl("https://kartta.hel.fi/wfs", "a:b"),
    );
    expect(
      pagingUrl(template("api-ckan"), { name: "", url: "", http: { url: "https://bcn.example/data/api/3/action/organization_list", preset: { kind: "ckan", organization: "ocupacio" } } }),
    ).toBe("https://bcn.example/data/api/3/action/package_search?fq=organization%3Aocupacio");
    expect(pagingWords({ paging: { mode: "offset" }, onPage: 200, total: 453, pages: 3, more: false })).toEqual({
      key: "pages",
      values: { pages: 3, onPage: 200, total: 453 },
    });
    expect(pagingWords({ paging: { mode: "offset" }, onPage: 60, more: true }).key).toBe("more");
    expect(pagingWords({ total: 9, onPage: 1, more: false }).key).toBe("totalOnly");
    expect(pagingWords({ more: false }).key).toBe("once");
    expect(pagingWords({ more: false, skipped: "the feed did not answer" }).key).toBe("skipped");
    expect(i18n.t("datasources.wizard.paging.pages", { pages: 14, onPage: 1000, total: 13500 })).toBe("Reads 14 pages of 1000 (13500 records).");
  });

  it("names a source after its address, as a DNS label", () => {
    expect(nameFrom("https://data.example.org/files/Kvalita_ovzdušia 2026.csv")).toBe("kvalita-ovzdusia-2026");
    expect(nameFrom("https://api.example.org/")).toBe("api-example-org");
    expect(nameFrom("not an address")).toBe("");
  });
});

describe("the template gallery", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("asks a WFS template for its layer, names the source after the address, and hands over the form", async () => {
    const user = userEvent.setup();
    const { onUse } = await renderWizard();
    const wfs = en.datasources.template.wfs;
    await user.click(screen.getByRole("button", { name: new RegExp(wfs.title) }));
    expect(screen.getByRole("button", { name: new RegExp(wfs.title) })).toHaveAttribute("aria-pressed", "true");
    await user.type(screen.getByLabelText(new RegExp(wfs.url)), "https://gis.example.org/parky-wfs");
    expect(screen.getByLabelText(/Name/)).toHaveValue("parky-wfs");
    await user.click(screen.getByRole("button", { name: en.datasources.template.use }));
    expect(screen.getByText(en.datasources.template.problem.layer)).toBeInTheDocument();
    expect(onUse).not.toHaveBeenCalled();

    await user.type(screen.getByLabelText(wfs.layer), "mesto:parky");
    await user.clear(screen.getByLabelText(/Name/));
    await user.type(screen.getByLabelText(/Name/), "parky");
    await user.click(screen.getByRole("button", { name: en.datasources.template.use }));
    expect(onUse).toHaveBeenCalledWith("http", {
      name: "parky",
      http: { url: wfsUrl("https://gis.example.org/parky-wfs", "mesto:parky"), verb: "GET" },
    });
  });

  it("asks a JSON API for the array its records sit in, refuses a path that is not names, and hands it over (T-6572)", async () => {
    const user = userEvent.setup();
    const { onUse } = await renderWizard();
    const json = en.datasources.template.json;
    await user.click(screen.getByRole("button", { name: (name) => name.startsWith(json.title) }));
    await user.type(screen.getByLabelText(new RegExp(json.url)), "https://gbfs.example.org/gbfs/station_information.json");
    const records = screen.getByRole("textbox", { name: en.datasources.field.records });
    expect(screen.getByText(en.datasources.field.recordsHelp)).toBeInTheDocument();
    await user.type(records, "data stations");
    await user.click(screen.getByRole("button", { name: en.datasources.template.use }));
    expect(screen.getByText(en.datasources.template.problem.records)).toBeInTheDocument();
    expect(onUse).not.toHaveBeenCalled();

    await user.clear(records);
    await user.type(records, "data.stations");
    await user.click(screen.getByRole("button", { name: en.datasources.template.use }));
    expect(onUse).toHaveBeenCalledWith("http", {
      name: "station-information",
      http: { url: "https://gbfs.example.org/gbfs/station_information.json", verb: "GET", records: "data.stations" },
    });
  });

  it("walks an open data API from its address to the form with no YAML: list, pick, pages, check (T-7094)", async () => {
    const user = userEvent.setup();
    const { onUse, asked, view } = await renderWizard((path) => {
      if (path.endsWith("/datasource-discovery")) {
        return json({
          field: "typeNames",
          url: "https://kartta.hel.fi/ws/geoserver/avoindata/wfs?service=WFS&version=2.0.0&request=GetCapabilities",
          choices: [{ value: "avoindata:Puut", label: "Trees" }],
          truncated: false,
        });
      }
      if (path.endsWith("/datasource-paging")) {
        return json({ paging: { mode: "offset", total: "numberMatched" }, records: "features", onPage: 200, total: 453, pages: 3, more: false });
      }
      return undefined;
    });
    await user.click(screen.getByRole("button", { name: new RegExp(en.datasources.preset.kinds.wfs) }));
    await user.type(screen.getByLabelText(new RegExp(en.datasources.wizard.apiUrl)), "https://kartta.hel.fi/ws/geoserver/avoindata/wfs");
    expect(screen.getByText(en.datasources.wizard.apiUrlHelp.wfs)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: en.datasources.discover.button }));
    await user.click(await screen.findByRole("button", { name: en.datasources.discover.use }));
    await user.click(screen.getByRole("button", { name: en.datasources.wizard.pagingButton }));
    await waitFor(() => expect(screen.getByTestId("wizard-paging")).toHaveTextContent("Reads 3 pages of 200 (453 records)."));
    expect(asked.find((a) => a.path.endsWith("/datasource-paging"))?.body).toEqual({
      url: wfsUrl("https://kartta.hel.fi/ws/geoserver/avoindata/wfs", "avoindata:Puut"),
    });
    await user.clear(screen.getByLabelText(/Name/));
    await user.type(screen.getByLabelText(/Name/), "trees");
    await user.click(screen.getByRole("button", { name: en.datasources.template.use }));
    expect(onUse).toHaveBeenCalledWith("http", {
      name: "trees",
      http: { url: "https://kartta.hel.fi/ws/geoserver/avoindata/wfs", preset: { kind: "wfs", typeNames: "avoindata:Puut" }, verb: "GET" },
    });
    await expectNoAxeViolations(view.container);
  });

  it("writes a JSON API's walk and records from its first answer, and a credential only as a picked secret (T-7094)", async () => {
    const user = userEvent.setup();
    const { onUse } = await renderWizard(
      (path) =>
        path.endsWith("/datasource-paging")
          ? json({ paging: { mode: "nextUrl", next: "@odata.nextLink" }, records: "value", onPage: 20, more: true })
          : undefined,
      ["northwind-key"],
    );
    await user.click(screen.getByRole("button", { name: (name) => name.startsWith(en.datasources.template.json.title) }));
    await user.type(screen.getByLabelText(new RegExp(en.datasources.template.json.url)), "https://services.example/Customers");
    // No password field anywhere: the credential is a secret's name and its key (PL-14).
    expect(document.querySelector('input[type="password"]')).toBeNull();
    await user.selectOptions(screen.getByLabelText(en.datasources.wizard.secret), "northwind-key");
    await user.click(screen.getByRole("button", { name: en.datasources.wizard.pagingButton }));
    await waitFor(() => expect(screen.getByTestId("wizard-paging")).toHaveTextContent("Reads pages of 20 until the service says there are no more."));
    expect(screen.getByRole("textbox", { name: en.datasources.field.records })).toHaveValue("value");
    await user.click(screen.getByRole("button", { name: en.datasources.template.use }));
    expect(screen.getByText(en.datasources.template.problem.secretKey)).toBeInTheDocument();
    expect(onUse).not.toHaveBeenCalled();
    await user.type(screen.getByLabelText(new RegExp(en.datasources.field.secretKey)), "token");
    await user.click(screen.getByRole("button", { name: en.datasources.template.use }));
    expect(onUse).toHaveBeenCalledWith("http", {
      name: "customers",
      http: {
        url: "https://services.example/Customers",
        verb: "GET",
        records: "value",
        paging: { mode: "nextUrl", next: "@odata.nextLink" },
        authorization: { headerRef: { name: "northwind-key", key: "token" } },
      },
    });
  });

  it("says why a guess failed, splits a CSV into rows by default, and opens another connection's own form (T-7094)", async () => {
    const user = userEvent.setup();
    const { onUse, onPick } = await renderWizard((path) =>
      path.endsWith("/datasource-paging") ? json({ more: false, skipped: "the feed did not answer within the test's three seconds" }) : undefined,
    );
    await user.click(screen.getByRole("button", { name: (name) => name.startsWith(en.datasources.template.json.title) }));
    await user.type(screen.getByLabelText(new RegExp(en.datasources.template.json.url)), "https://slow.example/x");
    await user.click(screen.getByRole("button", { name: en.datasources.wizard.pagingButton }));
    await waitFor(() => expect(screen.getByTestId("wizard-paging")).toHaveTextContent("Could not tell: the feed did not answer"));

    await user.click(screen.getByRole("button", { name: new RegExp(en.datasources.template.csv.title) }));
    expect(screen.getByText(en.datasources.wizard.oneRead)).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: new RegExp(en.datasources.wizard.rows) })).toBeChecked();
    // The address typed for the last card stays when another is chosen.
    const address = screen.getByLabelText(new RegExp(en.datasources.template.csv.url));
    expect(address).toHaveValue("https://slow.example/x");
    await user.clear(address);
    await user.type(address, "https://data.example.org/trees.csv");
    await user.click(screen.getByRole("button", { name: en.datasources.template.use }));
    expect(onUse).toHaveBeenCalledWith("http", { name: "trees", http: { url: "https://data.example.org/trees.csv", verb: "GET", format: "csv" } });

    await user.selectOptions(screen.getByLabelText(en.datasources.wizard.groups.other), "mqtt");
    expect(onPick).toHaveBeenCalledWith("mqtt");
  });
});
