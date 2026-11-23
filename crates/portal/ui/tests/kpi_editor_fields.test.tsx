/**
 * T-7680: every field of the KPI editor reaches the Pipeline it proposes. The only KPI target is
 * picked by itself; name, title, unit, schedule, target, each source's name, Endpoint, type,
 * attribute and key, the join, a typed formula, adding and removing a source; Escape closes; a
 * refused preview says why. The type and attribute pickers are stood in by plain selects: their
 * own suites cover the model lookups.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { KpiEditorDialog } from "../src/pages/pipelines/KpiEditor";
import { emptyDraft } from "../src/pages/pipelines/kpiEditor";
import type { KpiDraft } from "../src/pages/pipelines/kpiEditor";
import type { Manifest } from "../src/api/manifest";

vi.mock("../src/api/projects", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../src/api/projects")>()),
  useProjectOrganization: () => ({
    organization: { metadata: { name: "hel" }, spec: { domain: "hel.fi" } },
    settled: true,
  }),
}));

interface PickerProps {
  id?: string;
  label: string;
  value: string[];
  onChange: (names: string[]) => void;
}
const { picker } = vi.hoisted(() => ({
  picker:
    (options: string[]) =>
    ({ id, label, value, onChange }: PickerProps) => (
      <select
        id={id}
        aria-label={label}
        value={value[0] ?? ""}
        onChange={(e) => onChange(e.target.value ? [e.target.value] : [])}
      >
        <option value="">none</option>
        {options.map((one) => (
          <option key={one} value={one}>
            {one}
          </option>
        ))}
      </select>
    ),
}));
vi.mock("../src/components/pickers/TypePicker", () => ({
  TypePicker: picker(["ParkingSite", "TransitStop"]),
}));
vi.mock("../src/components/pickers/AttributePicker", () => ({
  AttributePicker: picker(["occupied", "capacity", "district"]),
}));

const k = en.pipelines.kpiEditor;
const TARGET = "urn:ngsi-ld:Endpoint:hel.fi:helsinki-kpi:kpi-writer";

const endpoint = (name: string, space: string): Manifest => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name, namespace: "helsinki" },
  spec: { contextSpaceRef: space },
});

function stubFetch(previewStatus = 200) {
  const json = (body: unknown, status = 200) =>
    Promise.resolve(
      new Response(JSON.stringify(body), {
        status,
        headers: { "Content-Type": "application/json" },
      }),
    );
  const fetchMock = vi.fn(
    async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = input instanceof Request ? input.url : String(input);
      if (url.includes("/kpis/check")) {
        const text =
          init?.body != null
            ? String(init.body)
            : input instanceof Request
              ? await input.clone().text()
              : "{}";
        const body = JSON.parse(text || "{}") as { preview?: boolean };
        if (body.preview && previewStatus !== 200) {
          return json(
            {
              type: "about:blank",
              title: "Forbidden",
              status: previewStatus,
              detail: "you may not read helsinki-parking",
            },
            previewStatus,
          );
        }
        return json({ problems: [], sources: [] });
      }
      if (url.includes("/endpoints")) {
        return json({
          items: [
            endpoint("helsinki-parking", "parking"),
            endpoint("helsinki-transit", "transit"),
            endpoint("kpi-writer", "helsinki-kpi"),
          ],
        });
      }
      if (url.includes("/permissions"))
        return json({ project: "helsinki", kinds: {} }, 404);
      return json({ items: [], models: [] });
    },
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function show(initial: KpiDraft = emptyDraft()) {
  const onPropose = vi.fn();
  const onClose = vi.fn();
  render(
    <QueryClientProvider
      client={
        new QueryClient({
          defaultOptions: {
            queries: { retry: false },
            mutations: { retry: false },
          },
        })
      }
    >
      <I18nextProvider i18n={i18n}>
        <KpiEditorDialog
          project="helsinki"
          initial={initial}
          pending={false}
          error={null}
          onClose={onClose}
          onPropose={onPropose}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { onPropose, onClose };
}

const propose = () =>
  screen.getByRole("button", { name: en.pipelines.propose });
const field = (id: string) =>
  document.getElementById(id) as HTMLInputElement | HTMLSelectElement;

describe("the KPI editor's fields", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "__Host-jc_csrf=csrf-token-1; Secure; path=/";
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("picks the only KPI target and proposes what each field was set to", async () => {
    stubFetch();
    const { onPropose } = show();
    await waitFor(() => expect(field("kpi-target")).toHaveValue(TARGET));
    expect(
      screen.getByRole("option", { name: "kpi-writer" }),
    ).toBeInTheDocument();

    await userEvent.type(field("kpi-name"), " stops-sum ");
    await userEvent.type(field("kpi-title"), "Stops");
    await userEvent.clear(field("kpi-unit"));
    await userEvent.type(field("kpi-unit"), "c62");
    expect(field("kpi-unit")).toHaveValue("C62");
    await userEvent.selectOptions(field("kpi-schedule"), "daily");
    // With one KPI target there is nothing else to pick: clearing it picks it again.
    await userEvent.selectOptions(field("kpi-target"), "");
    expect(field("kpi-target")).toHaveValue(TARGET);

    for (const [index, name, endpointName, type] of [
      [0, "Parking", "helsinki-parking", "ParkingSite"],
      [1, "Stops", "helsinki-transit", "TransitStop"],
    ] as const) {
      await userEvent.clear(field(`kpi-source-${index}-name`));
      await userEvent.type(field(`kpi-source-${index}-name`), name);
      await userEvent.selectOptions(
        field(`kpi-source-${index}-endpoint`),
        endpointName,
      );
      await userEvent.selectOptions(field(`kpi-source-${index}-type`), type);
    }
    await waitFor(() => expect(propose()).toBeEnabled());
    await userEvent.click(propose());
    const manifest = onPropose.mock.calls[0][0] as Manifest;
    expect(manifest.metadata.name).toBe("stops-sum");
    expect(manifest.spec).toMatchObject({
      schedule: "0 3 * * *",
      sources: [
        {
          name: "parking",
          endpointRef: { name: "helsinki-parking" },
          query: { type: "ParkingSite" },
        },
        {
          name: "stops",
          endpointRef: { name: "helsinki-transit" },
          query: { type: "TransitStop" },
        },
      ],
      steps: [
        { kpi: { title: "Stops", unitCode: "C62", formula: "count(parking)" } },
      ],
      outputs: [{ targetEndpoint: TARGET }],
    });
  });

  it("joins by an attribute each source names, and clears the type and attribute when the Endpoint changes", async () => {
    stubFetch();
    const draft: KpiDraft = {
      ...emptyDraft(TARGET),
      name: "per-stop",
      choice: "per",
      sources: [
        {
          name: "parking",
          endpoint: "helsinki-parking",
          type: "ParkingSite",
          attribute: "",
          key: "",
        },
        {
          name: "stops",
          endpoint: "helsinki-transit",
          type: "TransitStop",
          attribute: "",
          key: "",
        },
      ],
    };
    const { onPropose } = show(draft);
    expect(screen.queryByLabelText(k.key.area)).not.toBeInTheDocument();
    await userEvent.selectOptions(field("kpi-join"), "area");
    expect(screen.getAllByLabelText(k.key.area)).toHaveLength(2);
    await userEvent.selectOptions(field("kpi-source-0-attribute"), "occupied");
    await userEvent.selectOptions(field("kpi-source-0-key"), "district");
    await userEvent.selectOptions(field("kpi-source-1-key"), "district");
    await waitFor(() => expect(propose()).toBeEnabled());
    await userEvent.click(propose());
    expect((onPropose.mock.calls[0][0] as Manifest).spec).toMatchObject({
      steps: [
        {
          kpi: {
            formula: "sum(parking.occupied) / count(stops)",
            join: {
              by: "area",
              keys: { parking: "district", stops: "district" },
            },
          },
        },
      ],
    });

    await userEvent.selectOptions(field("kpi-source-0-key"), "");
    expect(field("kpi-source-0-key")).toHaveValue("");
    await userEvent.selectOptions(field("kpi-source-0-attribute"), "");
    expect(field("kpi-source-0-attribute")).toHaveValue("");
    await userEvent.selectOptions(field("kpi-source-0-type"), "");
    expect(field("kpi-source-0-type")).toHaveValue("");
    await userEvent.selectOptions(
      field("kpi-source-1-endpoint"),
      "helsinki-parking",
    );
    expect(field("kpi-source-1-type")).toHaveValue("");
    expect(field("kpi-source-1-key")).toHaveValue("");
  });

  it("adds sources up to five, removes one beyond the two a formula needs, and takes a typed formula", async () => {
    stubFetch();
    const { onPropose } = show({
      ...emptyDraft(TARGET),
      name: "free",
      sources: [
        {
          name: "parking",
          endpoint: "helsinki-parking",
          type: "ParkingSite",
          attribute: "occupied",
          key: "",
        },
        {
          name: "stops",
          endpoint: "helsinki-transit",
          type: "TransitStop",
          attribute: "",
          key: "",
        },
      ],
    });
    expect(
      screen.queryByRole("button", { name: /Remove source/ }),
    ).not.toBeInTheDocument();
    for (let i = 0; i < 3; i++)
      await userEvent.click(screen.getByRole("button", { name: k.addSource }));
    expect(screen.getByTestId("kpi-source-4")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: k.addSource }),
    ).not.toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: "Remove source source-4" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Remove source source-5" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Remove source source-3" }),
    );
    expect(screen.queryByTestId("kpi-source-2")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("switch", { name: k.advanced }));
    const formula = field("kpi-formula");
    expect(formula).toHaveValue("count(parking)");
    await userEvent.clear(formula);
    await userEvent.type(formula, "max(parking.occupied)");
    await waitFor(() => expect(propose()).toBeEnabled());
    await userEvent.click(propose());
    expect((onPropose.mock.calls[0][0] as Manifest).spec).toMatchObject({
      steps: [{ kpi: { formula: "max(parking.occupied)" } }],
    });
  });

  it("closes on Escape", async () => {
    stubFetch();
    const { onClose } = show();
    await userEvent.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalled();
  });

  it("says why a refused preview has no value", async () => {
    stubFetch(403);
    show({
      ...emptyDraft(TARGET),
      name: "count-parking",
      sources: [
        {
          name: "parking",
          endpoint: "helsinki-parking",
          type: "ParkingSite",
          attribute: "",
          key: "",
        },
        {
          name: "stops",
          endpoint: "helsinki-transit",
          type: "TransitStop",
          attribute: "",
          key: "",
        },
      ],
    });
    await waitFor(() => expect(propose()).toBeEnabled());
    await userEvent.click(screen.getByRole("button", { name: k.runPreview }));
    expect(
      await screen.findByText("you may not read helsinki-parking"),
    ).toBeInTheDocument();
  });
});
