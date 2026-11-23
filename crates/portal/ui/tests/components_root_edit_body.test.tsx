/** EditResourceDialogBody: closing, the editor mounting, and a refusal led by a field path (T-7585). */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";

const mounted = vi.fn();
vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: ({
    value,
    onChange,
    onMount,
  }: {
    value: string;
    onChange?: (value: string) => void;
    onMount?: () => void;
  }) => {
    mounted();
    onMount?.();
    return <textarea aria-label="YAML" value={value} onChange={(event) => onChange?.(event.target.value)} />;
  },
}));
vi.mock("@tanstack/react-router", () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a href="/x">{children}</a>,
}));

const { EditResourceDialogBody } = await import("../src/components/EditResourceDialogBody");

const PROJECT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Project",
  metadata: { name: "helsinki", namespace: "org", title: "Helsinki city data" },
  spec: { organizationRef: "hel", quotas: { contextSpaces: 4 } },
};
const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000002b", namespace: "org" },
  status: { lane: "yellow", phase: "PendingApproval" },
};
const TARGET = { project: "helsinki", home: "org", kind: "Project", plural: "projects", name: "helsinki" };

function reply(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" },
  });
}

function show(
  props: Partial<Parameters<typeof EditResourceDialogBody>[0]>,
  write: () => Response = () => reply(CHANGE, 200),
) {
  const sent: { method: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url, "http://localhost");
      if (request.method === "GET") return url.pathname.endsWith("/forms") ? reply({ items: [] }) : reply(PROJECT);
      sent.push({ method: request.method, body: await request.clone().json().catch(() => undefined) });
      return write();
    }),
  );
  const onOpenChange = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <EditResourceDialogBody target={TARGET} open onOpenChange={onOpenChange} {...props} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { sent, onOpenChange };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  mounted.mockClear();
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("the YAML editor", () => {
  it("mounts, Cancel closes it, and YAML that does not parse sends nothing", async () => {
    const user = userEvent.setup();
    const { sent, onOpenChange } = show({});
    const editor = await screen.findByLabelText("YAML");
    expect(mounted).toHaveBeenCalled();
    fireEvent.change(editor, { target: { value: "a: [1" } });
    const propose = screen.getByRole("button", { name: en.resourceEdit.propose });
    await user.click(propose);
    expect(sent).toEqual([]);
    await user.click(screen.getByRole("button", { name: en.form.cancel }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("proposes the stored manifest and Close follows the change", async () => {
    const user = userEvent.setup();
    const { sent, onOpenChange } = show({});
    await screen.findByLabelText("YAML");
    const propose = screen.getByRole("button", { name: en.resourceEdit.propose });
    await waitFor(() => expect(propose).toBeEnabled());
    await user.click(propose);
    expect(await screen.findByText(/chg-0000002b/)).toBeInTheDocument();
    expect(sent.every((one) => one.method === "PUT")).toBe(true);
    const closers = screen.getAllByRole("button", { name: en.resourceDelete.close });
    await user.click(closers[closers.length - 1]);
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("a reader gets the record closed with a Close button", async () => {
    const user = userEvent.setup();
    const { onOpenChange } = show({ readOnly: true });
    await screen.findByLabelText("YAML");
    const closers = screen.getAllByRole("button", { name: en.resourceDelete.close });
    await user.click(closers[closers.length - 1]);
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});

describe("the kind's form", () => {
  it("has a Cancel, and puts a refusal led by a field beside that field", async () => {
    const { projectSchema } = await import("../src/schemas/kinds");
    const { fromProject, toProject } = await import("../src/pages/projectSettings/ProjectSettingsPage");
    const user = userEvent.setup();
    const { sent, onOpenChange } = show(
      { form: { schema: projectSchema((key) => i18n.t(key)), fromManifest: fromProject, toManifest: toProject } },
      () =>
        reply(
          {
            title: "Unprocessable",
            status: 422,
            detail: "the manifest was refused",
            errors: ["spec.quotas.contextSpaces: above the organization's ceiling", "not led by a field", 7],
          },
          422,
        ),
    );
    const box = await screen.findByRole("dialog");
    await within(box).findByLabelText(/^Title/);
    await user.click(within(box).getAllByRole("button", { name: en.resourceEdit.propose }).at(-1) as HTMLElement);
    await waitFor(() => expect(sent.length).toBeGreaterThan(0));
    expect(await within(box).findByText("the manifest was refused")).toBeInTheDocument();
    expect(await within(box).findByText(/above the organization's ceiling/)).toBeInTheDocument();
    await user.click(within(box).getByRole("button", { name: en.form.cancel }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});
