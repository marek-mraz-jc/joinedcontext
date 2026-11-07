/**
 * T-1852: the pipeline canvas against the UI contract (UI-01, UI-15, UI-16, UI-30, UI-44).
 *
 * An SVG attribute takes a colour value, not a class, so the canvas was written with fifteen
 * literal colours and seven pixel font sizes — the one surface of the Portal that an
 * installation's brand and the dark theme never reached, drawing slate-grey boxes on a white
 * card inside a dark page. Every one of them is a token class now (`fill-surface`,
 * `stroke-danger`, `text-caption`), the node's focus ring is the Portal's own instead of
 * `outline-none`, and the step editor's textarea is the shared control.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { expectNoRawKeys, expectNoViolations } from "./checks";
import { PipelineFlow } from "../src/pages/pipelines/PipelineFlow";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import type { Trace } from "../src/pages/pipelines/PipelineTest";

vi.mock("../src/branding", () => ({ useBranding: () => ({ orgDomain: "hel.fi" }) }));

const FORM: PipelineForm = {
  name: "bikes",
  source: { dataSourceRef: "feed-bikes" },
  compute: { kind: "bloblang", bloblang: "root = this" },
  output: { type: "Vehicle", mode: "upsert" },
};

/** A run in which the mapping failed and the output never ran: three states on one canvas. */
const TRACE: Trace = {
  input: { events: 2, bytes: 40 },
  mapping: [],
  validation: [],
  errors: [{ stage: "mapping", step: 0, line: 1, message: "cannot parse the mapping" }],
};

function show(props: Partial<React.ComponentProps<typeof PipelineFlow>> = {}) {
  const view = render(
    <I18nextProvider i18n={i18n}>
      <PipelineFlow
        form={FORM}
        onChange={vi.fn()}
        trace={null}
        selected={null}
        onSelect={vi.fn()}
        dataSources={[]}
        endpoints={[]}
        {...props}
      />
    </I18nextProvider>,
  );
  return { container: view.container, user: userEvent.setup(), unmount: view.unmount };
}

/** Every colour and size a canvas element carries, as attributes and as inline style. */
function painted(container: HTMLElement): string[] {
  return [...container.querySelectorAll("svg *")].flatMap((node) =>
    ["fill", "stroke", "style"]
      .map((name) => node.getAttribute(name) ?? "")
      .filter((value) => value !== ""),
  );
}

describe("the pipeline canvas against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("paints with tokens: no literal colour and no pixel size anywhere on the canvas", () => {
    const { container } = show({ trace: TRACE });
    for (const value of painted(container)) {
      expect(value, `the canvas still paints with ${value}`).not.toMatch(/#[0-9a-f]{3,8}|rgba?\(|\d+px/i);
    }
    const rect = container.querySelector("svg rect");
    expect(rect?.getAttribute("class")).toContain("fill-surface");
  });

  it("gives a node the colour of its state, by token", () => {
    const { container } = show({ trace: TRACE });
    const strokeOf = (id: string) =>
      container.querySelector(`[data-testid="flow-node-${id}"] rect`)?.getAttribute("class") ?? "";
    // The mapping failed, the input before it did not, and nothing after it ran.
    expect(strokeOf("compute")).toContain("stroke-danger");
    expect(strokeOf("source")).toContain("stroke-success");

    const plain = show();
    expect(
      plain.container.querySelector('[data-testid="flow-node-source"] rect')?.getAttribute("class"),
    ).toContain("stroke-border");
  });

  it("marks the selected node in a tone of the brand a person can make out", () => {
    // Not the brand raw: an outline is a graphic, WCAG asks 3:1 of it, and a pale civic brand
    // left it at 1.67:1 on the page (T-2323, UI-30).
    const { container } = show({ selected: "output" });
    expect(container.querySelector('[data-testid="flow-node-output"] rect')?.getAttribute("class")).toContain(
      "stroke-primary-soft-fg",
    );
  });

  it("shows where the keyboard is: a node takes the focus and keeps the Portal's ring", async () => {
    show();
    const node = screen.getByTestId("flow-node-source");
    node.focus();
    expect(node).toHaveFocus();
    expect(node).toHaveClass("focus-ring");
    expect(node.getAttribute("class")).not.toContain("outline-none");
  });

  it("selects a node with Enter, as a pointer would", async () => {
    const onSelect = vi.fn();
    const { user } = show({ onSelect });
    screen.getByTestId("flow-node-output").focus();
    await user.keyboard("{Enter}");
    expect(onSelect).toHaveBeenCalledWith("output");
  });

  it("has no axe violation, with a trace painted on it and without", async () => {
    const plain = show();
    await expectNoViolations(plain.container);
    plain.unmount();
    const { container } = show({ trace: TRACE });
    await expectNoViolations(container);
  });

  it("shows no raw translation key in any locale the organisation offers", async () => {
    for (const locale of SUPPORTED_LOCALES) {
      await i18n.changeLanguage(locale);
      const { container, unmount } = render(
        <I18nextProvider i18n={i18n}>
          <PipelineFlow
          form={FORM}
          onChange={vi.fn()}
          trace={null}
          selected={null}
          onSelect={vi.fn()}
          dataSources={[]}
          endpoints={[]}
        />
        </I18nextProvider>,
      );
      expectNoRawKeys(container);
      unmount();
    }
    await i18n.changeLanguage("en");
  });
});
