/**
 * T-1611, UI-61, AG-77: a person inside a form can ask the assistant about the field they are stuck
 * on, and have the form filled in from a sentence — and what travels is where they are standing, never
 * what they typed.
 *
 * Before this, the dock knew the route and nothing else: a question asked with the pipeline form open
 * and the cursor in `spec.source.query.q` reached the model as bare prose, so "what goes here?" had no
 * "here". The form now names itself — the kind, the draft it edits, the field the action was clicked
 * on — and the assistant reads and writes that draft under the person's own grants.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ResourceFormDialog } from "../src/components/ResourceFormDialog";
// The dialog imports its body when it first opens (T-3316). Loaded here, with the file, the
// transform of the form engine is paid before any test starts, and each wait below waits for the
// dialog alone rather than for a cold import racing the wait's ceiling on a loaded host (T-3538).
import "../src/components/ResourceFormDialogBody";
import { formContext, onAskRequest, standingIn } from "../src/assistant/state";
import type { JsonSchema } from "../src/components/forms/types";

const SCHEMA: JsonSchema = {
  type: "object",
  required: ["name"],
  properties: {
    name: { type: "string", title: "Name" },
    url: { type: "string", title: "URL" },
  },
};

function renderDialog(kind = "DataSource", draftName = "hsl-citybikes") {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ResourceFormDialog
          open
          onOpenChange={() => {}}
          title="Data source"
          description="A data source"
          schema={SCHEMA}
          kind={kind}
          project="helsinki"
          draftKind={kind}
          draftName={draftName}
          submitLabel="Propose"
          onSubmit={() => {}}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  standingIn(null);
  vi.stubGlobal(
    "fetch",
    vi.fn(
      async () => new Response(JSON.stringify({ items: [] }), { status: 200 }),
    ),
  );
});

afterEach(() => {
  standingIn(null);
  vi.unstubAllGlobals();
});

describe("a form asks the assistant", () => {
  it("tells the assistant which form is open and which draft it edits", async () => {
    expect(
      formContext(),
      "nothing is open before a dialog renders",
    ).toBeUndefined();
    renderDialog();
    await waitFor(() => {
      expect(formContext()).toEqual({
        kind: "DataSource",
        name: "hsl-citybikes",
      });
    });
  });

  it("offers to fill the whole form from a sentence, and asks nothing by itself", async () => {
    const user = userEvent.setup();
    const asked: string[] = [];
    const stop = onAskRequest((question) => asked.push(question));
    renderDialog("Pipeline", "citybikes-gbfs");

    await user.click(
      await screen.findByRole("button", { name: en.form.fillFromSentence }),
    );
    expect(asked).toHaveLength(1);
    expect(asked[0]).toContain("Pipeline");
    // The sentence is unfinished on purpose: the person says what they want and sends it.
    expect(asked[0].trimEnd().endsWith(":")).toBe(true);
    stop();
  });

  it("takes the form back when the dialog closes, so a question from a page carries none", async () => {
    const { unmount } = render(
      <QueryClientProvider client={new QueryClient()}>
        <I18nextProvider i18n={i18n}>
          <ResourceFormDialog
            open
            onOpenChange={() => {}}
            title="Data source"
            description="A data source"
            schema={SCHEMA}
            kind="DataSource"
            project="helsinki"
            draftKind="DataSource"
            draftName="hsl-citybikes"
            submitLabel="Propose"
            onSubmit={() => {}}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await waitFor(() => expect(formContext()).toBeDefined());
    unmount();
    expect(formContext()).toBeUndefined();
  });
});
