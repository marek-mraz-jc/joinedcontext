/**
 * T-3280: the form engine loads with the first form opened, not with every page. The first form
 * waits for it with the page's loading state; once loaded, a form renders at once.
 */
import { render, screen } from "@testing-library/react";
import { Suspense } from "react";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm, preloadSchemaForm } from "../src/components/forms/SchemaFormLoader";
import { SchemaForm as DialogsForm } from "../src/components/forms/LazySchemaForm";
import { SchemaForm as Engine } from "../src/components/forms/SchemaForm";

const schema = { type: "object", properties: { name: { type: "string", title: "Name" } } } as const;

function show() {
  return render(
    <I18nextProvider i18n={i18n}>
      <Suspense fallback={<p>waiting for the form</p>}>
        <SchemaForm schema={schema} onSubmit={() => undefined} />
      </Suspense>
    </I18nextProvider>,
  );
}

describe("the lazily loaded form engine (T-3280)", () => {
  it("waits for the engine the first time, then draws the form", async () => {
    const { unmount } = show();
    expect(screen.getByText("waiting for the form")).toBeInTheDocument();
    expect(await screen.findByLabelText(/Name/)).toBeInTheDocument();
    unmount();
  });

  it("draws the form at once once the engine is loaded", async () => {
    await preloadSchemaForm();
    show();
    expect(screen.queryByText("waiting for the form")).toBeNull();
    expect(screen.getByLabelText(/Name/)).toBeInTheDocument();
  });

  it("hands the suite's dialogs the engine itself, so a test reads a form the moment it opens", () => {
    expect(DialogsForm).toBe(Engine);
  });
});
