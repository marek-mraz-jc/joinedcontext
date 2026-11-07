/**
 * Disabled with a reason (T-0603, UI-44, PF-50): a denied control stays, disabled, and names
 * the verb and the kind by pointer and by keyboard; an allowed one is untouched; no document
 * yet, or a bootstrap administrator, leaves it enabled.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/components/ui/PermissionGuard.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { Button, PermissionGuard } from "../src/components/ui";
import { DeleteResourceAction } from "../src/components/DeleteResourceDialog";
import { EditResourceAction } from "../src/components/EditResourceDialog";

const VIEWER = { project: "helsinki", bootstrap: false, grants: [] };
const EDITOR = {
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "endpoint-editor", binding: "editors", rule: { kinds: ["Endpoint"], verbs: ["propose"] } }],
};

function renderGuard(permissions: unknown, kind = "Endpoint", verb: "propose" | "delete" = "propose") {
  const fetchMock = vi.fn(() =>
    Promise.resolve(
      new Response(JSON.stringify(permissions), { status: 200, headers: { "Content-Type": "application/json" } }),
    ),
  );
  vi.stubGlobal("fetch", fetchMock);
  const onClick = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <PermissionGuard project="helsinki" kind={kind} verb={verb}>
          <Button onClick={onClick}>New endpoint</Button>
        </PermissionGuard>
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { fetchMock, onClick };
}

describe("the permission guard", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("disables a denied control, keeps it reachable and names the missing verb and kind", async () => {
    const { onClick } = renderGuard(VIEWER);
    // The control is re-rendered once the document arrives, so it is queried after the wait.
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /New endpoint/ })).toHaveAttribute(
        "title",
        expect.stringMatching(/^Disabled: /),
      );
    });
    const button = screen.getByRole("button", { name: /New endpoint/ });
    const reason = "Disabled: your role does not permit 'propose' on 'Endpoint' in this project";
    expect(button).toHaveAccessibleDescription(reason);
    expect(button).toHaveAttribute("title", reason);

    // `aria-disabled`, not `disabled`: a hard-disabled button leaves the tab order, and the
    // reason written for it is then unreachable — which is why this used to need a bare
    // `tabIndex={0}` span around it with no role, no name and no focus ring (T-1743).
    expect(button).not.toBeDisabled();
    button.focus();
    expect(button).toHaveFocus();

    await userEvent.click(button);
    expect(onClick).not.toHaveBeenCalled();
  });

  it("leaves an allowed control untouched", async () => {
    const { fetchMock, onClick } = renderGuard(EDITOR);
    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalled();
    });
    const button = screen.getByRole("button", { name: "New endpoint" });
    expect(button).toBeEnabled();
    expect(button).not.toHaveAttribute("aria-disabled");
    await userEvent.click(button);
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("guards by kind: the endpoint editor may not propose a pipeline", async () => {
    renderGuard(EDITOR, "Pipeline");
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /New endpoint/ })).toHaveAttribute(
        "title",
        expect.stringMatching(/^Disabled: /),
      );
    });
    expect(screen.getByRole("button", { name: /New endpoint/ })).toHaveAccessibleDescription(
      /'propose' on 'Pipeline'/,
    );
  });

  /// T-1142, PF-50: the guard reflects the document and decides nothing. A verb the role does
  /// not hold is closed even when a neighbouring verb on the same kind is open.
  it("closes the verb the role lacks and leaves the one it holds open", async () => {
    renderGuard(EDITOR, "Endpoint", "delete");
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /New endpoint/ })).toHaveAttribute(
        "title",
        expect.stringMatching(/^Disabled: /),
      );
    });
    expect(screen.getByRole("button", { name: /New endpoint/ })).toHaveAccessibleDescription(
      /'delete' on 'Endpoint'/,
    );

    // The same role, the verb it does hold.
    vi.unstubAllGlobals();
    renderGuard(EDITOR, "Endpoint", "propose");
    await waitFor(() => {
      expect(screen.getAllByRole("button", { name: /New endpoint/ }).at(-1)).toBeEnabled();
    });
  });

  // T-5833: the walker's viewer pressed "Add a source" on a slow dev, before the document said no.
  it("is disabled, saying it checks, while the document is on its way", async () => {
    let answer: (response: Response) => void = () => undefined;
    const fetchMock = vi.fn(() => new Promise<Response>((resolve) => (answer = resolve)));
    vi.stubGlobal("fetch", fetchMock);
    const onClick = vi.fn();
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <PermissionGuard project="helsinki" kind="Endpoint" verb="propose">
            <Button onClick={onClick}>New endpoint</Button>
          </PermissionGuard>
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalled();
    });
    const button = screen.getByRole("button", { name: /New endpoint/ });
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveAccessibleDescription(en.permissions.checking);
    await userEvent.click(button);
    expect(onClick).not.toHaveBeenCalled();

    answer(new Response(JSON.stringify(EDITOR), { status: 200, headers: { "Content-Type": "application/json" } }));
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "New endpoint" })).not.toHaveAttribute("aria-disabled");
    });
  });

  it("leaves the control to the API when the document could not be read", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(new Response('{"title":"Bad Gateway"}', { status: 502, headers: { "Content-Type": "application/problem+json" } }))),
    );
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <PermissionGuard project="helsinki" kind="Endpoint" verb="propose">
            <Button>New endpoint</Button>
          </PermissionGuard>
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "New endpoint" })).not.toHaveAttribute("aria-disabled");
    });
  });

  it("stays enabled for a bootstrap administrator", async () => {
    const { fetchMock } = renderGuard({ ...VIEWER, bootstrap: true });
    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalled();
    });
    expect(screen.getByRole("button", { name: /New endpoint/ })).toBeEnabled();
  });

  it("keeps a denied delete and edit on the row, disabled with the reason, and opens nothing", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() =>
        Promise.resolve(
          new Response(JSON.stringify(VIEWER), { status: 200, headers: { "Content-Type": "application/json" } }),
        ),
      ),
    );
    const target = { project: "helsinki", kind: "Endpoint", plural: "endpoints", name: "public-air" };
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <EditResourceAction target={target} />
          <DeleteResourceAction target={target} />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    for (const [name, verb] of [
      [/Edit/, "propose"],
      [new RegExp(en.resourceDelete.button), "delete"],
    ] as const) {
      const reason = `Disabled: your role does not permit '${verb}' on 'Endpoint' in this project`;
      await waitFor(() => {
        expect(screen.getByRole("button", { name })).toHaveAttribute("title", reason);
      });
      const button = screen.getByRole("button", { name });
      expect(button).toHaveAttribute("title", reason);
      expect(button).toHaveAccessibleDescription(reason);
      await userEvent.click(button);
    }
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
