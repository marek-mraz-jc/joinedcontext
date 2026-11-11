import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { WidgetProps } from "@rjsf/utils";
import "../src/i18n";
import en from "../src/locales/en.json";
import { RepresentationCheckboxes } from "../src/components/forms/widgets/RepresentationCheckboxes";
import { FormDataContext } from "../src/components/forms/widgets";

const OPTIONS = ["ngsi-ld", "csv", "mcp"].map((value) => ({ value, label: value.toUpperCase() }));

function boxes(audience: string, value: string[], onChange = vi.fn()) {
  const props = {
    id: "root_enabledRepresentations",
    name: "enabledRepresentations",
    value,
    onChange,
    onBlur: vi.fn(),
    onFocus: vi.fn(),
    options: { enumOptions: OPTIONS, enumDisabled: ["ngsi-ld", "mcp"] },
    schema: { type: "array", items: { type: "string", enum: OPTIONS.map((o) => o.value) }, uniqueItems: true },
    uiSchema: {},
    label: "Representations",
    required: false,
    disabled: false,
    readonly: false,
    autofocus: false,
    rawErrors: [],
    registry: {} as WidgetProps["registry"],
  } as unknown as WidgetProps;
  render(
    <FormDataContext.Provider value={{ audience }}>
      <RepresentationCheckboxes {...props} />
    </FormDataContext.Provider>,
  );
  return onChange;
}

/** T-6761 (EP-24): MCP on a public Endpoint is the person's switch, said under the boxes. */
describe("the representation checkboxes", () => {
  it("lock MCP on an internal Endpoint and say nothing more", () => {
    boxes("organization", ["ngsi-ld", "mcp"]);
    expect(screen.getByRole("checkbox", { name: "MCP" })).toBeDisabled();
    expect(screen.queryByTestId("public-mcp-hint")).not.toBeInTheDocument();
  });

  it("unlock MCP on a public Endpoint, say it is off, and turn it on when checked", async () => {
    const onChange = boxes("public", ["ngsi-ld"]);
    const mcp = screen.getByRole("checkbox", { name: "MCP" });
    expect(mcp).toBeEnabled();
    expect(mcp).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: "NGSI-LD" })).toBeDisabled();
    expect(screen.getByTestId("public-mcp-hint")).toHaveTextContent(en.endpoints.publicMcpHint);
    await userEvent.click(mcp);
    expect(onChange).toHaveBeenCalledWith(["ngsi-ld", "mcp"]);
  });

  it("say MCP is on and how to turn it off once checked", () => {
    boxes("public", ["ngsi-ld", "mcp"]);
    expect(screen.getByTestId("public-mcp-hint")).toHaveTextContent(en.endpoints.publicMcpOn);
  });
});
