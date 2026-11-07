/**
 * T-5052: the YAML view of a manifest. Monaco cannot run in jsdom, so the view is held at two
 * seams: the helpers that place a field, a description and an error on a line, and the props the
 * dialog hands the editor (read-only for a viewer, the marks of a refusal at its field's line).
 */
import { act, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { describe as describeField, lineOf, marksOf, parseErrorsOf, pathAt } from "../src/components/yamlManifest";

const editors: Record<string, unknown>[] = [];
vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: (props: Record<string, unknown>) => {
    editors.push(props);
    return <textarea aria-label="YAML" readOnly={Boolean(props.readOnly)} value={String(props.value)} />;
  },
}));

const { EditResourceDialogBody } = await import("../src/components/EditResourceDialogBody");

const TEXT = "apiVersion: joinedcontext.com/v1alpha1\nkind: Policy\nmetadata:\n  name: read\nspec:\n  rules:\n    - action: read\n";

const SCHEMA = {
  type: "object",
  properties: {
    spec: { $ref: "#/definitions/PolicySpec" },
  },
  definitions: {
    PolicySpec: {
      type: "object",
      properties: { rules: { description: "What the assignee may do.", type: "array", items: { type: "object" } } },
    },
  },
};

describe("the helpers of the YAML view", () => {
  it("finds the field under a position and the line of a field", () => {
    expect(pathAt(TEXT, TEXT.indexOf("rules"))).toEqual(["spec", "rules"]);
    expect(pathAt(TEXT, TEXT.indexOf("read\n", TEXT.indexOf("action")))).toEqual(["spec", "rules", 0, "action"]);
    expect(lineOf(TEXT, ["spec", "rules"])).toBe(6);
    expect(lineOf(TEXT, ["spec", "nothing"])).toBe(5);
  });

  it("reads a field's description through the schema's references", () => {
    expect(describeField(SCHEMA, ["spec", "rules"])).toBe("What the assignee may do.");
    expect(describeField(SCHEMA, ["spec", "unknown"])).toBeNull();
    expect(describeField(undefined, ["spec"])).toBeNull();
  });

  it("marks a parse error at its line and a refusal at its field's line", () => {
    expect(marksOf("a: 1\nb: [\n", [])[0]?.line).toBeGreaterThanOrEqual(2);
    expect(marksOf(TEXT, ["spec.rules: at least one rule is required"])).toEqual([
      { line: 6, message: "spec.rules: at least one rule is required" },
    ]);
    expect(marksOf(TEXT, ["the space is gone"])).toEqual([{ line: 1, message: "the space is gone" }]);
  });
});

function json(body: unknown) {
  return new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } });
}

function renderDialog(readOnly: boolean) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init);
      if (request.url.includes("/api/v1/kinds/Policy/schema")) {
        return json(SCHEMA);
      }
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Policy",
        metadata: { name: "read", namespace: "p" },
        spec: { rules: [{ action: "read" }] },
        status: { phase: "Live" },
      });
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <EditResourceDialogBody
          target={{ project: "p", kind: "Policy", plural: "policies", name: "read" }}
          open
          onOpenChange={() => {}}
          readOnly={readOnly}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
  editors.length = 0;
});

describe("the YAML view in the dialog", () => {
  it("a viewer reads the manifest in the editor, read-only, sections first and no status", async () => {
    renderDialog(true);
    const text = await within(await screen.findByRole("dialog")).findByRole("textbox", { name: "YAML" });
    expect(text).toHaveAttribute("readonly");
    const props = editors.at(-1) ?? {};
    expect(props.readOnly).toBe(true);
    expect(String(props.value).startsWith("apiVersion: ")).toBe(true);
    expect(String(props.value)).not.toContain("status:");
    expect(String(props.value)).toContain("  rules:\n    - action: read");
  });

  it("a steward edits it, and the hover describes a field from the kind's schema", async () => {
    renderDialog(false);
    await within(await screen.findByRole("dialog")).findByRole("textbox", { name: "YAML" });
    await vi.waitFor(() => {
      const describe = editors.at(-1)?.describeField as (p: (string | number)[]) => string | null;
      expect(describe(["spec", "rules"])).toBe("What the assignee may do.");
    });
    expect(editors.at(-1)?.readOnly).toBe(false);
  });

  it("text that does not parse names its line and reason and holds the proposal until it parses", async () => {
    renderDialog(false);
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByRole("textbox", { name: "YAML" });
    const propose = within(dialog).getByRole("button", { name: "Propose change" });
    const type = (next: string) => act(() => (editors.at(-1)?.onChange as (v: string) => void)(next));

    type("apiVersion: joinedcontext.com/v1alpha1\nkind: Policy\nmetadata:\n  name: read\nspec:\n  rules: [\n");
    const [error] = parseErrorsOf(String(editors.at(-1)?.value));
    expect(error?.line).toBeGreaterThanOrEqual(6);
    expect(error?.message).not.toMatch(/at line \d+/);
    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent(`Line ${error?.line}: ${error?.message}. Fix the YAML to propose the change.`);
    expect(propose).toHaveAttribute("aria-disabled", "true");

    type(TEXT);
    await vi.waitFor(() => expect(within(dialog).queryByRole("alert")).toBeNull());
    expect(propose).not.toHaveAttribute("aria-disabled", "true");
  });
});
