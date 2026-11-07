/**
 * The layout primitives (T-2777, UI-84): what they render and how they answer the keyboard. The
 * widths themselves are CSS and are checked in a real browser (e2e/responsive.spec.ts); here the
 * stylesheet is only held to defining a rule for every class the primitives render.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { Card, Grid, Header, Page, Sidebar, Split, Tabs } from "../src/sdk/layout";

const TABS = [
  { id: "map", label: "Map", render: () => <p>the map</p> },
  { id: "table", label: "Table", render: () => <p>the table</p> },
  { id: "chart", label: "Chart", render: () => <p>the chart</p> },
];

/** A key pressed where the focus is, the way a keyboard delivers it. */
function press(key: string): void {
  fireEvent.keyDown(document.activeElement ?? document.body, { key });
}

describe("Tabs", () => {
  it("shows the first tab, keeps only it in the tab order and labels the panel by it", () => {
    render(<Tabs tabs={TABS} label="Views" />);
    const map = screen.getByRole("tab", { name: "Map" });
    expect(map).toHaveAttribute("aria-selected", "true");
    expect(map).toHaveAttribute("tabindex", "0");
    expect(screen.getByRole("tab", { name: "Table" })).toHaveAttribute("tabindex", "-1");
    expect(screen.getByRole("tabpanel", { name: "Map" })).toHaveTextContent("the map");
    expect(screen.getByRole("tablist", { name: "Views" })).toBeInTheDocument();
  });

  it("moves with the arrow keys, wraps around, and jumps with Home and End", () => {
    const onChange = vi.fn();
    render(<Tabs tabs={TABS} label="Views" onChange={onChange} />);
    screen.getByRole("tab", { name: "Map" }).focus();
    press("ArrowRight");
    expect(screen.getByRole("tab", { name: "Table" })).toHaveFocus();
    expect(screen.getByRole("tabpanel")).toHaveTextContent("the table");
    press("End");
    expect(screen.getByRole("tab", { name: "Chart" })).toHaveFocus();
    press("ArrowRight");
    expect(screen.getByRole("tab", { name: "Map" })).toHaveFocus();
    press("ArrowLeft");
    expect(screen.getByRole("tab", { name: "Chart" })).toHaveAttribute("aria-selected", "true");
    press("Home");
    expect(screen.getByRole("tabpanel")).toHaveTextContent("the map");
    expect(onChange.mock.calls.map(([id]) => id)).toEqual(["table", "chart", "map", "chart", "map"]);
  });

  it("opens on a known initial tab, falls back to the first for an unknown one, and renders nothing without tabs", () => {
    const { unmount } = render(<Tabs tabs={TABS} label="Views" initial="chart" />);
    expect(screen.getByRole("tabpanel")).toHaveTextContent("the chart");
    unmount();
    render(<Tabs tabs={TABS} label="Views" initial="nope" />);
    expect(screen.getByRole("tabpanel")).toHaveTextContent("the map");
    const { container } = render(<Tabs tabs={[]} label="None" />);
    expect(container).toBeEmptyDOMElement();
  });
});

describe("Sidebar", () => {
  it("opens the drawer from a button that names it, moves focus in, and Escape gives it back", () => {
    render(
      <Sidebar label="Filters" side={<p>the filters</p>}>
        <p>the content</p>
      </Sidebar>,
    );
    const toggle = screen.getByRole("button", { name: "Filters" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByRole("complementary", { name: "Filters" })).toHaveTextContent("the filters");
    expect(screen.getByText("the content")).toBeInTheDocument();

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("complementary", { name: "Filters" })).toHaveFocus();

    press("Escape");
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveFocus();
  });

  it("closes from its own close button", () => {
    render(
      <Sidebar label="Layers" side={<p>layers</p>} position="end">
        <p>map</p>
      </Sidebar>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Layers" }));
    fireEvent.click(screen.getByRole("button", { name: "Close Layers" }));
    expect(screen.getByRole("button", { name: "Layers" })).toHaveAttribute("aria-expanded", "false");
  });
});

describe("Page, Header, Card, Grid and Split", () => {
  it("render their content under the headings and landmarks a screen reader reads", () => {
    render(
      <Page label="Stations" width="narrow">
        <Header title="Bike stations" subtitle="Live" level={1} actions={<button type="button">Export</button>} />
        <Grid columns={4}>
          <Card title="Bikes" actions={<button type="button">Open</button>}>
            12
          </Card>
          <Card label="Docks">7</Card>
        </Grid>
        <Split ratio="2:1">
          <p>left</p>
          <p>right</p>
        </Split>
      </Page>,
    );
    expect(screen.getByRole("region", { name: "Stations" })).toHaveAttribute("data-width", "narrow");
    expect(screen.getByRole("heading", { level: 1, name: "Bike stations" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 2, name: "Bikes" })).toBeInTheDocument();
    expect(screen.getByRole("article", { name: "Docks" })).toHaveTextContent("7");
    expect(screen.getByText("left").closest(".jc-split")).toHaveAttribute("data-ratio", "2:1");
    expect(screen.getByText("12").closest(".jc-grid-cols")).toHaveAttribute("data-columns", "4");
  });
});

describe("layout.css", () => {
  const css = readFileSync(join(__dirname, "..", "src", "sdk", "layout.css"), "utf8");
  const source = readFileSync(join(__dirname, "..", "src", "sdk", "layout.tsx"), "utf8");

  it("defines a rule for every class the primitives render", () => {
    const used = new Set([...source.matchAll(/className="([^"]+)"/g)].flatMap((match) => match[1].split(/\s+/)));
    const missing = [...used].filter((name) => !css.includes(`.${name}`));
    expect(missing).toEqual([]);
  });

  it("styles the shell every App renders, so an App without a stylesheet of its own still looks like the others", () => {
    const shell = readFileSync(join(__dirname, "..", "src", "sdk", "shell.tsx"), "utf8");
    const used = new Set([...shell.matchAll(/className="([^"]+)"/g)].flatMap((match) => match[1].split(/\s+/)));
    expect([...used].filter((name) => !css.includes(`.${name} `) && !css.includes(`.${name},`))).toEqual([]);
  });

  it("falls back to the system's colours in the shell and the panel, so an App without tokens stays readable dark or light", () => {
    const start = css.indexOf(".jc-shell {");
    const end = css.indexOf(".jc-panel-actions");
    const own = css.slice(start, end);
    // A fixed light colour as the fallback paints dark text on an App's dark card (axe colour contrast).
    expect(own).not.toMatch(/var\(--jc-color-(ink|surface|card|muted|danger|success), #/);
  });

  it("widens by the box's own width and keeps a 44 px target on a phone", () => {
    expect(css).toMatch(/\.jc-grid-box,\s*\.jc-split-box\s*\{\s*container-type: inline-size;/);
    expect(css.match(/@container \(width >= \d+rem\)/g)).toHaveLength(4);
    expect(css).toContain("--jc-target: 2.75rem;");
    expect(css).toContain("@media (pointer: coarse), (width < 40rem)");
  });
});
