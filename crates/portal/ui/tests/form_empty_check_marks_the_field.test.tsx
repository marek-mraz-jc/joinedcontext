import { useState } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
// The dialog itself, not its lazy shell: these cases read the form the moment it renders.
import { ResourceFormDialogBody as ResourceFormDialog } from "../src/components/ResourceFormDialogBody";
import type { JsonSchema } from "../src/components/forms/types";
import { DNS1123 } from "../src/schemas/kinds";

// Monaco draws on a canvas and starts a worker, neither of which exists in jsdom.
vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: ({ value, onChange }: { value: string; onChange?: (value: string) => void }) => (
    <textarea aria-label="YAML" value={value} onChange={(event) => onChange?.(event.target.value)} />
  ),
}));

// UI-44, UI-45: an empty form's Check is answered where the field is, in words, by the browser —
// the server's own sentence is a fallback, not the first thing a person reads (T-1491).
const SCHEMA: JsonSchema = {
  type: "object",
  required: ["name"],
  properties: {
    name: { type: "string", title: "Name", pattern: DNS1123 },
    audience: { type: "string", title: "Audience" },
  },
};

interface Form {
  name?: string;
  audience?: string;
}

/** Held by the page, the way every kind's page holds it. */
function Harness({ onCheck }: { onCheck: (form: Form) => void }): React.JSX.Element {
  const [form, setForm] = useState<Form>({});
  return (
    <ResourceFormDialog<Form>
      open
      onOpenChange={() => {}}
      title="Endpoint"
      description="An endpoint"
      schema={SCHEMA}
      formData={form}
      onChange={(next) => setForm(next ?? {})}
      submitLabel="Propose"
      onCheck={onCheck}
      onSubmit={() => {}}
    />
  );
}

/** The same dialog, handed a red verdict whose finding names a path. */
function openWithFinding(): void {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ResourceFormDialog<Form>
          open
          onOpenChange={() => {}}
          title="Endpoint"
          description="An endpoint"
          schema={SCHEMA}
          formData={{ name: "air-quality" }}
          submitLabel="Propose"
          onCheck={() => {}}
          verdict={{
            ok: false,
            findings: [{ level: "error", path: "metadata.name", message: "A name is taken already" }],
            checkedAt: new Date().toISOString(),
            inputDigest: "sha256:0",
          }}
          onSubmit={() => {}}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

function open(onCheck: (form: Form) => void): void {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <Harness onCheck={onCheck} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("an empty check marks the name and says what a name is", () => {
  it("marks the field and says the rule in words, and asks the server nothing it cannot read", async () => {
    const onCheck = vi.fn();
    open(onCheck);

    await userEvent.click(screen.getByRole("button", { name: /check/i }));

    const name = await screen.findByLabelText(/Name/);
    await waitFor(() => expect(name).toHaveAttribute("aria-invalid", "true"));
    // The field says what a name is, and no regular expression and no envelope wording reaches it.
    // The sentence is where a screen reader reads it: on the field's own description.
    const described = (name.getAttribute("aria-describedby") ?? "")
      .split(" ")
      .map((id) => document.getElementById(id)?.textContent ?? "")
      .join(" ");
    expect(described).toContain(i18n.t("form.required"));
    expect(document.body.textContent ?? "").not.toContain("^[a-z0-9]");
    expect(document.body.textContent ?? "").not.toContain("resource envelope");
    // A required field is empty, so the manifest would not parse: the field says what it needs
    // and nothing is sent (T-2634). A form the schema refuses for anything else is still checked
    // by the server, which sees what the schema cannot (the test below and the next one).
    expect(onCheck).not.toHaveBeenCalled();
  });

  it("asks the server once the form can pass the schema", async () => {
    const onCheck = vi.fn();
    open(onCheck);

    await userEvent.type(await screen.findByLabelText(/Name/), "air-quality");
    await userEvent.click(screen.getByRole("button", { name: /check/i }));

    await waitFor(() => expect(onCheck).toHaveBeenCalledTimes(1));
    expect(await screen.findByLabelText(/Name/)).not.toHaveAttribute("aria-invalid", "true");
  });

  it("still asks the server when the fields are there and one of them breaks a rule", async () => {
    const onCheck = vi.fn();
    open(onCheck);

    await userEvent.type(await screen.findByLabelText(/Name/), "Air Quality");
    await userEvent.click(screen.getByRole("button", { name: /check/i }));

    await waitFor(() => expect(onCheck).toHaveBeenCalledTimes(1));
    expect(await screen.findByLabelText(/Name/)).toHaveAttribute("aria-invalid", "true");
  });

  it("lands a server finding on the field its path names", async () => {
    openWithFinding();

    const name = await screen.findByLabelText(/Name/);
    await waitFor(() => expect(name).toHaveAttribute("aria-invalid", "true"));
    const described = (name.getAttribute("aria-describedby") ?? "")
      .split(" ")
      .map((id) => document.getElementById(id)?.textContent ?? "")
      .join(" ");
    expect(described).toContain("A name is taken already");
  });
});
