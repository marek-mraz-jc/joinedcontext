import { useState } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
// The dialog itself, not its lazy shell: these cases read the form the moment it renders.
import { ResourceFormDialogBody as ResourceFormDialog } from "../src/components/ResourceFormDialogBody";
import type { JsonSchema } from "../src/components/forms/types";

vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: ({ value }: { value: string }) => <textarea aria-label="YAML" value={value} readOnly />,
}));

// T-3970: a Check that stops on an empty required field names it beside the button and brings
// its control into view, opening the fold it sits in. T-3969: a refusal the kind passes stops
// the Check the same way, naming the control that fixes it.
const SCHEMA: JsonSchema = {
  type: "object",
  required: ["name", "target"],
  properties: {
    name: { type: "string", title: "Name" },
    target: { type: "string", title: "Target endpoint" },
    note: { type: "string", title: "Note" },
  },
};

interface Form {
  name?: string;
  target?: string;
  note?: string;
}

function Harness({
  onCheck,
  initial,
  refusal,
}: {
  onCheck: (form: Form) => void;
  initial: Form;
  refusal?: { message: string; field?: string };
}): React.JSX.Element {
  const [form, setForm] = useState<Form>(initial);
  return (
    <ResourceFormDialog<Form>
      open
      onOpenChange={() => {}}
      title="Pipeline"
      description="A pipeline"
      schema={SCHEMA}
      // The target is chosen in a workbench beside the form, as the pipeline dialog does it.
      uiSchema={{ target: { "ui:widget": "hidden" } }}
      formData={form}
      onChange={(next) => setForm(next ?? {})}
      submitLabel="Propose"
      onCheck={onCheck}
      checkRefusal={refusal}
      onSubmit={() => {}}
    >
      <details data-testid="fold">
        <summary>More options</summary>
        <select
          aria-label="Target endpoint"
          data-field="target"
          value={form.target ?? ""}
          onChange={(event) => setForm({ ...form, target: event.target.value || undefined })}
        >
          <option value="">Choose</option>
          <option value="urn:ngsi-ld:Endpoint:hel.fi:helsinki:all">all</option>
        </select>
      </details>
    </ResourceFormDialog>
  );
}

function open(onCheck: (form: Form) => void, initial: Form, refusal?: { message: string; field?: string }): void {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <Harness onCheck={onCheck} initial={initial} refusal={refusal} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("a stopped check says which field and brings it into view", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("opens the closed fold, focuses the control and names the field, and sends nothing", async () => {
    const onCheck = vi.fn();
    open(onCheck, { name: "no-target" });
    expect(screen.getByTestId("fold")).not.toHaveAttribute("open");

    await userEvent.click(screen.getByRole("button", { name: "Check" }));

    expect(await screen.findByTestId("check-stopped")).toHaveTextContent("Target endpoint is required");
    await waitFor(() => expect(screen.getByLabelText("Target endpoint")).toHaveFocus());
    expect(screen.getByTestId("fold")).toHaveAttribute("open");
    expect(onCheck).not.toHaveBeenCalled();
  });

  it("focuses a field of the form itself, and the line goes once the draft changes", async () => {
    const onCheck = vi.fn();
    open(onCheck, { target: "urn:ngsi-ld:Endpoint:hel.fi:helsinki:all" });

    await userEvent.click(screen.getByRole("button", { name: "Check" }));

    expect(await screen.findByTestId("check-stopped")).toHaveTextContent("Name is required");
    const name = screen.getByRole("textbox", { name: /^Name/ });
    await waitFor(() => expect(name).toHaveFocus());
    await userEvent.type(name, "x");
    expect(screen.queryByTestId("check-stopped")).toBeNull();
  });

  it("stops on the kind's refusal even when the schema is satisfied, naming its control", async () => {
    const onCheck = vi.fn();
    const refusal = { message: "The pipeline has no first source.", field: "target" };
    open(onCheck, { name: "drawn", target: "urn:ngsi-ld:Endpoint:hel.fi:helsinki:all" }, refusal);

    await userEvent.click(screen.getByRole("button", { name: "Check" }));

    expect(await screen.findByTestId("check-stopped")).toHaveTextContent("The pipeline has no first source.");
    await waitFor(() => expect(screen.getByLabelText("Target endpoint")).toHaveFocus());
    expect(onCheck).not.toHaveBeenCalled();
  });

  it("checks a complete form with no refusal and says nothing", async () => {
    const onCheck = vi.fn();
    open(onCheck, { name: "whole", target: "urn:ngsi-ld:Endpoint:hel.fi:helsinki:all" });

    await userEvent.click(screen.getByRole("button", { name: "Check" }));

    await waitFor(() => expect(onCheck).toHaveBeenCalledTimes(1));
    expect(screen.queryByTestId("check-stopped")).toBeNull();
  });
});
