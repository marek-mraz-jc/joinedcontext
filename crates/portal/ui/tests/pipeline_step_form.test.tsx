/**
 * T-3088: every processor step the palette offers has a form generated from the pinned runner's
 * own field tree, the YAML stays the second view of the same block, a list of processors inside
 * a step is a YAML box that refuses at its own field, and a credential takes a `${VAR}`.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/pipelines/processorForm.ts.
import { useState } from "react";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { StepForm } from "../src/pages/pipelines/PipelineEditor";
import { PROCESSOR_GROUPS, StepBlock } from "../src/pages/pipelines/PipelineFlow";
import type { ProcessorForms } from "../src/pages/pipelines/processorForm";
import { errorAt, fromFormData, toFormData } from "../src/pages/pipelines/processorForm";
import { YamlFieldError } from "../src/schemas/kinds";
import formsJson from "../src/schemas/bento-processor-forms.json";
import { expectNoViolations } from "./checks";

const forms = (formsJson as unknown as ProcessorForms).processors;

describe("the generated forms", () => {
  it("has a form for every processor the palette offers, and nothing else", () => {
    const palette = PROCESSOR_GROUPS.flatMap(({ processors }) => processors.map(({ name }) => name));
    expect(Object.keys(forms).sort()).toEqual([...palette].sort());
  });

  it("requires what the runner requires and offers a closed list as a select", () => {
    const dedupe = forms.dedupe.schema;
    expect(dedupe.required).toEqual(["cache", "key"]);
    expect((dedupe.properties?.strategy as { enum?: string[] }).enum).toEqual(["FIFO", "LIFO"]);
  });

  it("asks a credential for a variable, the ones the runner does not flag included", () => {
    const http = forms.http.schema.properties as Record<string, { properties: Record<string, { pattern?: string }> }>;
    for (const field of [http.basic_auth.properties.password, http.oauth.properties.access_token]) {
      expect(new RegExp(field.pattern ?? "").test("${FEED_TOKEN}")).toBe(true);
      expect(new RegExp(field.pattern ?? "").test("hunter2")).toBe(false);
    }
  });
});

describe("form data and the runner's config", () => {
  const cases: [string, unknown][] = [
    ["dedupe", { cache: "seen", key: "${! this.id }", strategy: "LIFO" }],
    ["switch", [{ check: 'this.type == "a"', processors: [{ log: { message: "a" } }] }, { processors: [{ mapping: "root = deleted()" }] }]],
    ["branch", { request_map: "root = this.id", processors: [{ http: { url: "https://x.example" } }], result_map: "root.got = this" }],
    ["catch", [{ log: { message: "failed" } }]],
    ["mapping", "root = this"],
    ["http", { url: "https://x.example", headers: { Accept: "application/json" }, basic_auth: { enabled: true, password: "${PW}" } }],
  ];

  it.each(cases)("round-trips a %s step through its form unchanged", (name, config) => {
    const { schema, uiSchema } = forms[name];
    const held = toFormData(config, schema, uiSchema);
    expect(fromFormData(held, schema, uiSchema)).toEqual(config);
  });

  it("holds a nested list of processors as YAML text and leaves an emptied box out", () => {
    const { schema, uiSchema } = forms.branch;
    const held = toFormData({ processors: [{ log: { message: "x" } }] }, schema, uiSchema) as Record<string, unknown>;
    expect(held.processors).toBe("- log:\n    message: x");
    expect(fromFormData({ processors: "  " }, schema, uiSchema)).toEqual({});
  });

  it("names the box that does not parse, inside a case of a switch too", () => {
    const { schema, uiSchema } = forms.switch;
    let thrown: unknown;
    try {
      fromFormData([{ check: "true", processors: "- log: [" }], schema, uiSchema);
    } catch (error) {
      thrown = error;
    }
    expect(thrown).toBeInstanceOf(YamlFieldError);
    expect((thrown as YamlFieldError).field).toBe("0.processors");
    expect(errorAt("0.processors", "bad")).toEqual({ 0: { processors: { __errors: ["bad"] } } });
    expect(errorAt("", "bad")).toEqual({ __errors: ["bad"] });
  });
});

function Harness({ initial, seen }: { initial: StepForm; seen: (entry: StepForm) => void }) {
  const [entry, setEntry] = useState(initial);
  return (
    <QueryClientProvider client={new QueryClient()}>
      <I18nextProvider i18n={i18n}>
        <StepBlock
          entry={entry}
          onChange={(next) => {
            seen(next);
            setEntry(next);
          }}
          onRemove={() => undefined}
        />
      </I18nextProvider>
    </QueryClientProvider>
  );
}

describe("the step's form", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("opens on the form, takes a field, and the YAML view shows the same block", async () => {
    const seen = vi.fn();
    render(<Harness initial={{ step: { processor: { dedupe: { cache: "seen", key: "${! this.id }" } } } }} seen={seen} />);
    const panel = await screen.findByTestId("flow-step-form");
    expect(screen.getByRole("tab", { name: en.pipelines.flow.stepForm })).toHaveAttribute("aria-selected", "true");
    const cache = within(panel).getByRole("textbox", { name: /cache/ });
    expect(cache).toHaveValue("seen");
    await expectNoViolations(panel);

    fireEvent.change(cache, { target: { value: "dedupe-cache" } });
    await waitFor(() =>
      expect(seen).toHaveBeenLastCalledWith({
        step: { processor: { dedupe: expect.objectContaining({ cache: "dedupe-cache", key: "${! this.id }" }) } },
      }),
    );

    await userEvent.click(screen.getByRole("tab", { name: en.pipelines.flow.stepYamlTab }));
    expect((screen.getByTestId("flow-step-yaml") as HTMLTextAreaElement).value).toContain("cache: dedupe-cache");
  });

  it("starts the form from what was typed in the YAML view", async () => {
    render(<Harness initial={{ step: { processor: { jq: { query: ".id" } } } }} seen={vi.fn()} />);
    await userEvent.click(await screen.findByRole("tab", { name: en.pipelines.flow.stepYamlTab }));
    fireEvent.change(screen.getByTestId("flow-step-yaml"), { target: { value: "jq:\n  query: .name\n" } });
    await userEvent.click(screen.getByRole("tab", { name: en.pipelines.flow.stepForm }));
    expect(within(await screen.findByTestId("flow-step-form")).getByRole("textbox", { name: /query/ })).toHaveValue(".name");
  });

  it("refuses a nested list that is not YAML at its own box and keeps the last good block", async () => {
    const seen = vi.fn();
    render(
      <Harness
        initial={{ step: { processor: { branch: { processors: [{ log: { message: "x" } }] } } } }}
        seen={seen}
      />,
    );
    const panel = await screen.findByTestId("flow-step-form");
    const box = within(panel).getByRole("textbox", { name: /processors/ });
    expect(box).toHaveValue("- log:\n    message: x");
    fireEvent.change(box, { target: { value: "- log: [" } });
    await waitFor(() => expect(box).toHaveAttribute("aria-invalid", "true"));
    expect(within(panel).getByText(/This is not YAML yet/)).toBeInTheDocument();
    expect(seen).not.toHaveBeenCalled();
    // The box keeps what the author typed rather than snapping back.
    expect(box).toHaveValue("- log: [");
  });

  it("says at the field that a credential takes a variable", async () => {
    render(
      <Harness
        initial={{ step: { processor: { http: { url: "https://x.example", basic_auth: { enabled: true, password: "${PW}" } } } } }}
        seen={vi.fn()}
      />,
    );
    const panel = await screen.findByTestId("flow-step-form");
    const password = panel.querySelector("#root_basic_auth_password") as HTMLInputElement;
    expect(password).toHaveAccessibleDescription(/name a variable of the pipeline's secrets, as \$\{VAR\}/);
    fireEvent.change(password, { target: { value: "hunter2" } });
    await waitFor(() => expect(password).toHaveAttribute("aria-invalid", "true"));
  });

  it("moves between the two views from the keyboard", async () => {
    render(<Harness initial={{ step: { processor: { jq: { query: ".id" } } } }} seen={vi.fn()} />);
    const formTab = await screen.findByRole("tab", { name: en.pipelines.flow.stepForm });
    formTab.focus();
    await userEvent.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: en.pipelines.flow.stepYamlTab })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByTestId("flow-step-yaml")).toBeInTheDocument();
  });
});
