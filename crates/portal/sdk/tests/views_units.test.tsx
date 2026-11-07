/**
 * T-2812 (DM-06): the app views write a quantity with its model's unit, read from the schema's
 * `x-unit`, since a key-value row carries none.
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Detail } from "../src/views/Detail";
import { Stats } from "../src/views/Stats";
import { Table } from "../src/views/Table";
import { unitsOf } from "../src/write";

const ROWS = [
  { id: "urn:ngsi-ld:AirQualityObserved:hel.fi:air:1", type: "AirQualityObserved", pm10: 12, name: "Kallio" },
  { id: "urn:ngsi-ld:AirQualityObserved:hel.fi:air:2", type: "AirQualityObserved", pm10: 18, name: "Mannerheimintie" },
];
const UNITS = unitsOf({
  properties: {
    pm10: { type: "number", "x-unit": { exactMappings: ["ucefact:GQ"] } },
    name: { type: "string" },
  },
});

describe("units in the app views", () => {
  it("reads the unit of each quantity from the schema", () => {
    expect(UNITS).toEqual({ pm10: "GQ" });
    expect(unitsOf(undefined)).toEqual({});
  });

  it("puts the model's unit on an average, not on a count, and lets the spec's own unit win", () => {
    render(
      <Stats
        rows={ROWS}
        units={UNITS}
        items={[
          { label: "Average PM10", agg: "avg", attr: "pm10" },
          { label: "Stations", agg: "count" },
          { label: "Worst", agg: "max", attr: "pm10", unit: "ug" },
        ]}
      />,
    );
    expect(screen.getByText("Average PM10").previousElementSibling).toHaveTextContent("15 µg/m³");
    expect(screen.getByText("Stations").previousElementSibling).toHaveTextContent(/^2$/);
    expect(screen.getByText("Worst").previousElementSibling).toHaveTextContent("18 ug");
  });

  it("names the unit in the table header and beside a detail value", () => {
    const { unmount } = render(<Table rows={ROWS} columns={["name", "pm10"]} selected={null} onSelect={() => {}} units={UNITS} />);
    expect(screen.getByRole("columnheader", { name: /pm10 \(µg\/m³\)/ })).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: /^name/ })).not.toHaveTextContent("(");
    unmount();
    render(<Detail row={ROWS[0]} attrs={["name", "pm10"]} units={UNITS} />);
    expect(screen.getByText("12 µg/m³")).toBeInTheDocument();
    expect(screen.getByText("Kallio")).toBeInTheDocument();
  });
});
