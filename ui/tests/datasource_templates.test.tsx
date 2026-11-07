/** T-3249: data source templates fill an existing type's form from what a person types. */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { TemplateGallery } from "../src/pages/datasources/TemplateGallery";
import { TEMPLATES, fromTemplate, nameFrom, problemOf, wfsUrl } from "../src/pages/datasources/templates";

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
    const onUse = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <TemplateGallery onUse={onUse} />
      </I18nextProvider>,
    );
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
    const onUse = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <TemplateGallery onUse={onUse} />
      </I18nextProvider>,
    );
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
});
