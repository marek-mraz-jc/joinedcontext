/**
 * T-7585: the widgets of the forms folder the other tests never drive: the colour and SVG widgets,
 * a multiple select, the entity type picker, a typed suggestion and the operations' sort order.
 */
import type { ReactNode } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { WidgetProps } from "@rjsf/utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import type { JsonSchema, UiSchema } from "../src/components/forms/types";
import { FormProjectContext, TypePickerWidget } from "../src/components/forms/widgets/ModelWidgets";
import { AssigneePicker } from "../src/components/forms/widgets/SuggestWidgets";
import { OperationsPicker } from "../src/components/forms/widgets/OperationsPicker";

function wrap(node: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>{node}</QueryClientProvider>
    </I18nextProvider>,
  );
}

function widgetProps(id: string, extra: Partial<WidgetProps> = {}): WidgetProps {
  return {
    id,
    name: id,
    value: undefined,
    required: false,
    disabled: false,
    readonly: false,
    autofocus: false,
    label: id,
    options: {},
    schema: { type: "string" },
    onChange: vi.fn(),
    onBlur: vi.fn(),
    onFocus: vi.fn(),
    registry: {} as WidgetProps["registry"],
    ...extra,
  };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the appearance form's colour and SVG fields", () => {
  const schema: JsonSchema = {
    type: "object",
    properties: {
      primary: { type: "string", title: "Primary colour" },
      logo: { type: "string", title: "Logo" },
    },
  };
  const ui: UiSchema = { primary: { "ui:widget": "colour" }, logo: { "ui:widget": "svgImage" } };

  it("paints the colour beside its field and previews the pasted SVG as an image", () => {
    const { container } = wrap(
      <SchemaForm
        schema={schema}
        uiSchema={ui}
        formData={{ primary: "#0b5394", logo: '<svg viewBox="0 0 1 1"><rect/></svg>' }}
        onSubmit={() => undefined}
      />,
    );
    expect(container.querySelector("rect")?.getAttribute("fill")).toBe("#0b5394");
    expect(screen.getByLabelText(/Primary colour/)).toHaveValue("#0b5394");
    expect(screen.getByRole("img", { name: en.form.svgPreview.replace("{label}", "Logo") })).toBeInTheDocument();
    expect(screen.getByText(en.form.svgPreviewNote)).toBeInTheDocument();
  });

  it("shows no preview for a value that is no SVG and no swatch fill for a value that is no colour", () => {
    const { container } = wrap(
      <SchemaForm schema={schema} uiSchema={ui} formData={{ primary: 4, logo: "<html></html>" }} onSubmit={() => undefined} />,
    );
    expect(screen.queryByRole("img")).toBeNull();
    expect(screen.queryByText(en.form.svgPreviewNote)).toBeNull();
    expect(container.querySelector("rect")?.getAttribute("fill")).toBeNull();
  });
});

describe("a select that takes several values", () => {
  it("answers the list of every selected option", async () => {
    const changed: unknown[] = [];
    const schema: JsonSchema = {
      type: "object",
      properties: { tags: { type: "array", title: "Tags", items: { type: "string", enum: ["a", "b", "c"] }, uniqueItems: true } },
    };
    wrap(
      <SchemaForm
        schema={schema}
        uiSchema={{ tags: { "ui:widget": "select" } }}
        formData={{}}
        onSubmit={() => undefined}
        onChange={(data) => changed.push(data)}
      />,
    );
    await userEvent.selectOptions(screen.getByLabelText(/Tags/), ["a", "c"]);
    expect(changed.at(-1)).toEqual({ tags: ["a", "c"] });
  });
});

describe("the entity type picker widget", () => {
  it("writes the one type picked", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" || input instanceof URL ? String(input) : input.url, window.location.origin);
        if (url.pathname === "/api/v1/organization/datamodels") {
          return Response.json({
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "List",
            items: [
              { name: "mobility", level: "project", project: "helsinki", space: "mobility", version: "0.3.0", lifecycle: "draft", classes: ["Road"] },
            ],
            smartDataModels: [],
          });
        }
        return new Response("not found", { status: 404 });
      }),
    );
    const props = widgetProps("root_type", { label: "Entity type" });
    wrap(
      <FormProjectContext.Provider value="helsinki">
        <TypePickerWidget {...props} />
      </FormProjectContext.Provider>,
    );
    await userEvent.click(screen.getByRole("combobox"));
    await userEvent.click(await screen.findByRole("option", { name: /Road/ }));
    await waitFor(() => expect(props.onChange).toHaveBeenCalledWith("Road"));
  });
});

describe("a suggested field typed into", () => {
  it("writes what is typed and nothing at all once it is emptied", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => Response.json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] })));
    const typed = widgetProps("root_assignee_id", { value: "x" });
    wrap(<AssigneePicker {...typed} />);
    const box = screen.getByRole("textbox");
    await userEvent.type(box, "y");
    expect(typed.onChange).toHaveBeenLastCalledWith("xy");
    await userEvent.clear(box);
    expect(typed.onChange).toHaveBeenLastCalledWith(undefined);
    await userEvent.tab();
    expect(typed.onBlur).toHaveBeenCalledWith("root_assignee_id", "x");
  });
});

describe("the operations a policy grants, as the manifest orders them", () => {
  it("writes the groups first, then the individual operations by name", async () => {
    const props = widgetProps("root_operations", { value: ["zeta", "alpha"], required: true, schema: { type: "array" } });
    const { container } = wrap(<OperationsPicker {...props} />);
    const box = container.querySelector("#root_operations-retrieveOps");
    expect(box).not.toBeNull();
    await userEvent.click(box as Element);
    expect(props.onChange).toHaveBeenCalledWith(["retrieveOps", "alpha", "zeta"]);
  });
});
