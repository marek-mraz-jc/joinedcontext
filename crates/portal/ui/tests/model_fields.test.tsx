/**
 * T-5066 (DM-61, DM-74): a model page's Fields tab lists every field with what the model allows
 * beside what the model's space holds, from the daily quality run, with a link to query it. A
 * viewer reads the same facts and finds no way to edit.
 */
import { screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import { jsonResponse, list, renderRoute } from "./pageHarness";
import { allowedOf } from "../src/pages/models/ModelFields";
import { parseModel } from "../src/pages/models/linkml";
import type { Manifest } from "../src/api/manifest";

const manifest = (kind: string, name: string, spec: Record<string, unknown>) =>
  ({ apiVersion: "joinedcontext.com/v1alpha1", kind, metadata: { name, namespace: "helsinki" }, spec }) as Manifest;

const AIR = `id: https://hel.fi/models/air
name: air
classes:
  AirQualityObserved:
    slots: [pm10, level, code]
slots:
  pm10:
    range: float
    required: true
    minimum_value: 0
    maximum_value: 500
    description: Particles under 10 µm.
    unit: { ucum_code: ug/m3, symbol: µg/m³ }
  level: { range: AirLevel }
  code: { range: string, pattern: "^[A-Z]{2}[0-9]{3}$" }
enums:
  AirLevel:
    permissible_values:
      good: { title: { en: Good } }
      bad: {}
`;

const LISTS: Record<string, Manifest[]> = {
  datamodels: [manifest("DataModel", "air", { contextSpaceRef: "ilma", linkml: AIR, version: "1.0.0", classes: ["AirQualityObserved"] })],
  spaces: [manifest("ContextSpace", "ilma", {})],
};

const REPORT = {
  observedAt: "2026-10-21T03:00:00Z",
  checked: 40,
  invalid: 0,
  truncated: false,
  rules: [],
  freshness: [],
  types: [
    {
      type: "AirQualityObserved",
      count: 40,
      newest: "2026-10-21T02:55:00Z",
      attributes: [
        { name: "pm10", present: 40, min: 3.5, max: 88, outliers: 0, outlierExamples: [] },
        { name: "level", present: 30, outliers: 0, outlierExamples: [] },
      ],
    },
  ],
};

function answer(quality: Response | null) {
  return (path: string): Response | undefined => {
    const plural = /^\/api\/v1\/projects\/helsinki\/([a-z]+)$/.exec(path)?.[1];
    if (plural) return jsonResponse(list(LISTS[plural] ?? []));
    if (path === "/api/v1/projects/helsinki/spaces/ilma/quality") return quality ?? jsonResponse(null);
    return undefined;
  };
}

const READER = { project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["*"], verbs: ["read"] } }] };

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the Fields tab of a model's page", () => {
  it("shows what each field allows beside what the space holds, with a query link", async () => {
    await renderRoute({ path: "/projects/helsinki/models/air?tab=fields", answer: answer(jsonResponse(REPORT)) });
    const table = await screen.findByRole("table", { name: en.models.fields.caption.replace("{type}", "AirQualityObserved") });
    const pm10 = within(table).getByText("pm10").closest("tr") as HTMLElement;
    expect(pm10).toHaveTextContent("0 – 500");
    expect(pm10).toHaveTextContent("µg/m³");
    expect(pm10).toHaveTextContent("Particles under 10 µm.");
    expect(await within(pm10).findByText("100%")).toBeInTheDocument();
    expect(pm10).toHaveTextContent("seen 3.5 – 88");
    const level = within(table).getByText("level").closest("tr") as HTMLElement;
    expect(level).toHaveTextContent("good (Good), bad");
    expect(within(level).getByText("75%")).toBeInTheDocument();
    const code = within(table).getByText("code").closest("tr") as HTMLElement;
    expect(code).toHaveTextContent("^[A-Z]{2}[0-9]{3}$");
    expect(code).toHaveTextContent("0%");
    const query = within(level).getByRole("link", { name: "Open Explore on the AirQualityObserved entities that have level" });
    expect(query).toHaveAttribute("href", "/projects/helsinki/explore?space=ilma&type=AirQualityObserved&q=level");
    expect(screen.getByText(/Facts from the space ilma/)).toBeInTheDocument();
  });

  it("names the rules the daily run found broken on a field", async () => {
    const broken = { ...REPORT, invalid: 3, rules: [{ rule: "MaxInclusive", path: "pm10", count: 3, examples: [] }] };
    await renderRoute({ path: "/projects/helsinki/models/air?tab=fields", answer: answer(jsonResponse(broken)) });
    const table = await screen.findByRole("table", { name: en.models.fields.caption.replace("{type}", "AirQualityObserved") });
    const pm10 = within(table).getByText("pm10").closest("tr") as HTMLElement;
    expect(await within(pm10).findByText("3 entities break MaxInclusive")).toBeInTheDocument();
    const level = within(table).getByText("level").closest("tr") as HTMLElement;
    expect(level).not.toHaveTextContent("break");
  });

  it("says when the daily run has not read the space yet", async () => {
    await renderRoute({ path: "/projects/helsinki/models/air?tab=fields", answer: answer(null) });
    expect(await screen.findByText(en.models.fields.notYet.replace("{space}", "ilma"))).toBeInTheDocument();
    const table = screen.getByRole("table", { name: en.models.fields.caption.replace("{type}", "AirQualityObserved") });
    expect(within(table).getByText("pm10").closest("tr")).toHaveTextContent("—");
  });

  it("names the reason when the facts cannot be read, and still lists the fields", async () => {
    const refused = jsonResponse({ title: "Forbidden", status: 403, detail: "no policy grants reading ilma" }, 403);
    await renderRoute({ path: "/projects/helsinki/models/air?tab=fields", answer: answer(refused) });
    expect(await screen.findByRole("alert")).toHaveTextContent("no policy grants reading ilma");
    expect(screen.getByRole("table", { name: en.models.fields.caption.replace("{type}", "AirQualityObserved") })).toBeInTheDocument();
  });

  it("gives a viewer the fields and the facts and no way to edit", async () => {
    await renderRoute({ path: "/projects/helsinki/models/air?tab=fields", answer: answer(jsonResponse(REPORT)), permissions: READER });
    const table = await screen.findByRole("table", { name: en.models.fields.caption.replace("{type}", "AirQualityObserved") });
    expect(await within(table).findByText("100%")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.models.page.addField })).toBeNull();
    expect(screen.queryByRole("link", { name: en.models.page.edit })).toBeNull();
    // Removal is refused with its reason, as Edit is (UI-44), never a button that does nothing.
    const remove = screen.getByRole("button", { name: en.resourceDelete.action.replace("{name}", "air") });
    expect(remove).toHaveAttribute("aria-disabled", "true");
    expect(remove).toHaveAccessibleDescription(/does not permit 'delete' on 'DataModel'/);
  });

  it("lets a steward propose removing the model from its page, the name typed back first", async () => {
    await renderRoute({ path: "/projects/helsinki/models/air", answer: answer(jsonResponse(REPORT)) });
    (await screen.findByRole("button", { name: en.resourceDelete.action.replace("{name}", "air") })).click();
    const dialog = await screen.findByRole("dialog", { name: en.resourceDelete.title.replace("{name}", "air") });
    expect(within(dialog).getByRole("button", { name: en.resourceDelete.propose })).toHaveAttribute("aria-disabled", "true");
  });
});

describe("allowedOf", () => {
  it("spells enum values, bounds, a pattern and a unit, and nothing for a free field", () => {
    const model = parseModel(AIR);
    const slot = (name: string) => model.slots.find((one) => one.name === name)!;
    expect(allowedOf(slot("pm10"), model, "en")).toEqual(["0 – 500", "µg/m³"]);
    expect(allowedOf(slot("level"), model, "en")).toEqual(["good (Good), bad"]);
    expect(allowedOf(slot("code"), model, "en")).toEqual(["^[A-Z]{2}[0-9]{3}$"]);
    expect(allowedOf({ ...slot("code"), pattern: undefined }, model, "en")).toEqual([]);
  });
});
