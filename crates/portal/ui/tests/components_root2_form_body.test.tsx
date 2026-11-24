/**
 * The form dialog's own paths the other form tests leave out: the refusal for a role that may not
 * create, a tab pressed that is already open, a YAML manifest the schema refuses, and a Check
 * that stops on a required field hidden among the advanced ones.
 */
import { useState } from "react";
import { render, screen, waitFor, within, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { doorsAsRest } from "./odata";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { BrandingProvider } from "../src/branding";
import { ResourceFormDialog } from "../src/components/ResourceFormDialog";
import type { ManifestSource } from "../src/components/ResourceFormDialog";
import type { JsonSchema } from "../src/components/forms/types";
import { DNS1123 } from "../src/schemas/kinds";
import { OTHER_BRAND } from "./page_contract";

vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: ({ value, onChange }: { value: string; onChange?: (value: string) => void }) => (
    <textarea aria-label="YAML" value={value} onChange={(event) => onChange?.(event.target.value)} />
  ),
}));

interface Space {
  name?: string;
  description?: string;
}

const SCHEMA: JsonSchema = {
  type: "object",
  required: ["name", "description"],
  properties: {
    name: { type: "string", title: "Name", pattern: DNS1123 },
    description: { type: "string", title: "Description" },
  },
};

const SOURCE: ManifestSource<Space> = {
  toManifest: (form) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: form.name ?? "" }, spec: { description: form.description ?? "" } }),
  fromManifest: (manifest) => {
    const m = manifest as { metadata?: { name?: string }; spec?: { description?: string } };
    return { name: m.metadata?.name, description: m.spec?.description };
  },
};

const FORM = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "UiSchema",
  metadata: { name: "testspace", namespace: "org" },
  spec: { for: "TestSpace", fields: { description: { advanced: true } } },
};

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

let grants: unknown[];
let sent: string[];

function show(props: { lockedName?: string; kind?: string; onOpenChange?: (open: boolean) => void; initial?: Space; onSubmit?: (form: Space) => void; withSource?: boolean }) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
      const path = new URL(request.url).pathname;
      if (request.method !== "GET") sent.push(`${request.method} ${path}`);
      if (path === "/api/v1/branding") return json(OTHER_BRAND);
      if (path.endsWith("/permissions/me")) return json({ project: "helsinki", bootstrap: false, grants });
      if (path.endsWith("/preferences")) return json({});
      if (path.endsWith("/forms")) return json(list([FORM]));
      if (path.includes("/drafts/")) return json({ title: "Not Found", status: 404 }, 404);
      return json(list([]));
    }),
  );
  doorsAsRest();
  function Page(): React.JSX.Element {
    const [data, setData] = useState<Space>(props.initial ?? {});
    return (
      <ResourceFormDialog<Space>
        open
        onOpenChange={props.onOpenChange ?? (() => {})}
        title="New context space"
        description="A place for the data of one purpose."
        schema={SCHEMA}
        kind={props.kind ?? "TestSpace"}
        lockedName={props.lockedName}
        project="helsinki"
        formData={data}
        onChange={(next) => setData(next ?? {})}
        submitLabel="Propose space"
        source={props.withSource ? SOURCE : undefined}
        onCheck={() => {}}
        onSubmit={props.onSubmit ?? (() => {})}
      />
    );
  }
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <BrandingProvider>
          <Page />
        </BrandingProvider>
      </I18nextProvider>
    </QueryClientProvider>,
  );
}


describe("the form dialog, further paths", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    grants = [{ rule: { kinds: ["TestSpace"], verbs: ["propose"] } }];
    sent = [];
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("tells a role that may not create so, and Back to list closes the dialog", async () => {
    grants = [];
    const closed = vi.fn();
    show({ onOpenChange: closed });
    expect(await screen.findByText(en.form.mayNotCreate)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: en.form.backToList }));
    expect(closed).toHaveBeenCalledWith(false);
  });

  it("pressing the tab that is already open changes nothing", async () => {
    show({ withSource: true, initial: { name: "air", description: "x" } });
    const form = await screen.findByRole("tab", { name: en.form.view.form });
    expect(form).toHaveAttribute("aria-selected", "true");
    await userEvent.click(form);
    expect(form).toHaveAttribute("aria-selected", "true");
    expect(screen.getByLabelText(/Name/)).toHaveValue("air");
  });

  it("a YAML manifest the schema refuses is listed and not proposed", async () => {
    const onSubmit = vi.fn();
    show({ withSource: true, initial: { name: "air", description: "x" }, onSubmit });
    await userEvent.click(await screen.findByRole("tab", { name: en.form.view.yaml }));
    const yaml = await screen.findByRole("textbox", { name: "YAML" });
    fireEvent.change(yaml, {
      target: { value: "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: Not Valid\nspec:\n  description: x\n" },
    });
    await userEvent.click(await screen.findByRole("button", { name: "Propose space" }));
    const alert = await screen.findByText(en.form.schemaErrors);
    expect(within(alert.closest("[role=alert]") as HTMLElement).getAllByRole("listitem").length).toBeGreaterThan(0);
    expect(onSubmit).not.toHaveBeenCalled();
    expect(sent).toEqual([]);
  });

  it("Check on a form missing a required advanced field shows the advanced fields", async () => {
    show({ initial: { name: "air" } });
    await within(await screen.findByRole("dialog")).findByLabelText(/Name/);
    expect(screen.queryByLabelText(/Description/)).toBeNull();
    await waitFor(() => expect(screen.getByRole("button", { name: en.form.check })).not.toHaveAttribute("aria-disabled", "true"));
    await userEvent.click(screen.getByRole("button", { name: en.form.check }));
    expect(await screen.findByLabelText(/Description/)).toBeInTheDocument();
  });

  const VALID = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: water\nspec:\n  description: Water\n";

  it("leaving the YAML view carries what was typed there into the form", async () => {
    show({ withSource: true, initial: { name: "air", description: "x" } });
    await userEvent.click(await screen.findByRole("tab", { name: en.form.view.yaml }));
    fireEvent.change(await screen.findByRole("textbox", { name: "YAML" }), { target: { value: VALID } });
    await userEvent.click(screen.getByRole("tab", { name: en.form.view.form }));
    expect(await screen.findByLabelText(/Name/)).toHaveValue("water");
  });

  it("YAML that does not parse keeps the person in the YAML view and says why, on leaving and on Propose", async () => {
    const onSubmit = vi.fn();
    show({ withSource: true, initial: { name: "air", description: "x" }, onSubmit });
    await userEvent.click(await screen.findByRole("tab", { name: en.form.view.yaml }));
    fireEvent.change(await screen.findByRole("textbox", { name: "YAML" }), { target: { value: "metadata: [unclosed" } });
    await userEvent.click(screen.getByRole("tab", { name: en.form.view.form }));
    expect(screen.getByRole("tab", { name: en.form.view.yaml })).toHaveAttribute("aria-selected", "true");
    expect(await screen.findAllByRole("alert")).not.toHaveLength(0);
    await userEvent.click(screen.getByRole("button", { name: "Propose space" }));
    expect(onSubmit).not.toHaveBeenCalled();
    expect(screen.getByRole("tab", { name: en.form.view.yaml })).toHaveAttribute("aria-selected", "true");
  });

  it("a YAML rename of an existing resource is refused with its name", async () => {
    const onSubmit = vi.fn();
    show({ withSource: true, lockedName: "air", initial: { name: "air", description: "x" }, onSubmit });
    await userEvent.click(await screen.findByRole("tab", { name: en.form.view.yaml }));
    fireEvent.change(await screen.findByRole("textbox", { name: "YAML" }), { target: { value: VALID } });
    await userEvent.click(screen.getByRole("button", { name: "Propose space" }));
    expect(await screen.findByText(en.resourceEdit.renamed.replace("{name}", "air"))).toBeInTheDocument();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("a second submit while the first is being sent proposes once", async () => {
    const onSubmit = vi.fn();
    show({ initial: { name: "air", description: "x" }, onSubmit });
    const propose = await screen.findByRole("button", { name: "Propose space" });
    await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled", "true"));
    // Enter in a field submits the form even while the button is busy.
    const form = propose.closest("form") as HTMLFormElement;
    fireEvent.submit(form);
    fireEvent.submit(form);
    await waitFor(() => expect(onSubmit).toHaveBeenCalled());
    expect(onSubmit).toHaveBeenCalledTimes(1);
  });
});
