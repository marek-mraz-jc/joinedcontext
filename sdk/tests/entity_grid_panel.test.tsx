import { describe, expect, it } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource } from "../src/grid/source";
import { EntitySelectionProvider, useEntitySelection } from "../src/sdk/panel";

const stations = [
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001",
    type: "BikeHireDockingStation",
    name: { type: "LanguageProperty", languageMap: { en: "Kamppi" } },
    availableBikeNumber: { type: "Property", value: 5 },
  },
];

const config = parseGridConfig({
  source: { kind: "fixture", name: "test" },
  type: "BikeHireDockingStation",
  columns: [
    { attr: "name", label: "Name", pinned: true },
    { attr: "availableBikeNumber", label: "Bikes" },
  ],
  pageSize: 10,
}).config!;

/** What the shell's panel would show: the selected entity, as the shell's selection holds it. */
function Selected() {
  const { selected } = useEntitySelection();
  return <output aria-label="selected">{selected ? `${selected.type} ${selected.id}` : "none"}</output>;
}

// SDK-40, T-3396: one panel per App. A grid inside an App's shell opens a row there, so a grid row
// opens the same panel as a map feature or a card; outside a shell (the Portal's data views) the
// grid keeps its own row detail.
describe("a grid row opened inside and outside an App's shell", () => {
  it("selects the row's entity for the shell's panel, and shows no detail of its own", async () => {
    render(
      <EntitySelectionProvider>
        <EntityGrid config={config} source={fixtureSource(stations)} />
        <Selected />
      </EntitySelectionProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: /Kamppi/ }));
    expect(screen.getByLabelText("selected")).toHaveTextContent("BikeHireDockingStation urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("opens its own row detail where no shell holds a panel", async () => {
    render(<EntityGrid config={config} source={fixtureSource(stations)} />);
    fireEvent.click(await screen.findByRole("button", { name: /Kamppi/ }));
    // The grid's own detail: a modal dialog headed by the row's name.
    await waitFor(() => expect(screen.getByRole("dialog", { name: /Kamppi/ })).toBeInTheDocument());
  });
});
