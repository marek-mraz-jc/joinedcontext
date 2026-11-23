/**
 * T-7585: the small layout and grid pieces whose last lines were never run: the lazy shell, the
 * public frame's Sign in, the workspace context outside a provider, the schema widgets of a
 * nullable or plain enum, and the read-only YAML dialog.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Suspense } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import type { Manifest } from "../src/api/manifest";
import { fieldWidget } from "../src/components/grid/fieldWidget";
import { ManifestYamlDialog } from "../src/components/grid/ManifestYamlDialog";
// vite.config.ts maps the literal LazyShell path to Shell itself; a computed path reaches the lazy module.
const lazyShellPath = "../src/components/layout/" + "LazyShell.tsx";
const { Shell } = (await import(/* @vite-ignore */ lazyShellPath)) as typeof import("../src/components/layout/LazyShell");
import { PublicFrame } from "../src/components/layout/PublicFrame";
import { WorkspaceProvider, setActiveWorkspace, useWorkspace, workspaceMiddleware } from "../src/components/layout/WorkspaceContext";

vi.mock("../src/components/layout/Shell", () => ({
  Shell: () => <p>the signed-in shell</p>,
}));

const signIn = vi.hoisted(() => vi.fn());
vi.mock("../src/auth/AuthProvider", () => ({
  useAuth: () => ({ status: "anonymous", signIn }),
}));

vi.mock("@tanstack/react-router", () => ({
  useNavigate: () => vi.fn(),
  useRouterState: ({ select }: { select: (s: { location: { search: unknown } }) => unknown }) =>
    select({ location: { search: {} } }),
}));

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  signIn.mockReset();
  window.history.pushState({}, "", "/");
});

describe("LazyShell", () => {
  it("loads the shell with the first signed-in page", async () => {
    render(
      <Suspense fallback={<p>loading</p>}>
        <Shell project="helsinki">
          <p>page</p>
        </Shell>
      </Suspense>,
    );
    expect(await screen.findByText("the signed-in shell")).toBeInTheDocument();
  });
});

describe("PublicFrame", () => {
  it("sends a visitor who presses Sign in back to the same address", async () => {
    window.history.pushState({}, "", "/glossary?term=a");
    render(
      <PublicFrame>
        <p>page</p>
      </PublicFrame>,
    );
    await userEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(signIn).toHaveBeenCalledWith("/glossary?term=a");
  });

  it("shows no Sign in on a page marked anonymous", () => {
    render(
      <PublicFrame anonymous>
        <p>page</p>
      </PublicFrame>,
    );
    expect(screen.queryByRole("button", { name: "Sign in" })).toBeNull();
  });
});

describe("useWorkspace outside a provider", () => {
  it("is in the project, and entering or leaving does nothing", async () => {
    function Probe(): React.JSX.Element {
      const { name, enter, leave } = useWorkspace();
      return (
        <div>
          <p>{name ?? "the project"}</p>
          <button type="button" onClick={() => enter("x")}>
            enter
          </button>
          <button type="button" onClick={leave}>
            leave
          </button>
        </div>
      );
    }
    render(<Probe />);
    await userEvent.click(screen.getByRole("button", { name: "enter" }));
    await userEvent.click(screen.getByRole("button", { name: "leave" }));
    expect(screen.getByText("the project")).toBeInTheDocument();
    expect(WorkspaceProvider).toBeTypeOf("function");
  });
});

describe("fieldWidget on nullable and plain enums", () => {
  const schema = {
    type: "object",
    properties: {
      spec: {
        type: "object",
        properties: {
          mode: { anyOf: [{ type: "string", enum: ["a", "b"] }, { type: "null" }] },
          plain: { type: "string", enum: ["x", "y", 3] },
          nested: { type: "object" },
        },
      },
    },
  };

  it("marks an enum with a null branch as nullable", () => {
    expect(fieldWidget("Endpoint", schema, "mode")?.widget).toEqual({
      kind: "enum",
      options: [{ value: "a" }, { value: "b" }],
      nullable: true,
    });
  });

  it("offers the string values of a plain enum only", () => {
    expect(fieldWidget("Endpoint", schema, "plain")?.widget).toEqual({
      kind: "enum",
      options: [{ value: "x" }, { value: "y" }],
      nullable: false,
    });
  });

  it("leaves a structure to the full form", () => {
    expect(fieldWidget("Endpoint", schema, "nested")).toBeNull();
  });
});

describe("ManifestYamlDialog", () => {
  const manifest = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "air", namespace: "helsinki" },
    spec: { hosts: ["a.example", "b.example"], password: "hunter2" },
  } as unknown as Manifest;

  it("shows the stored manifest with the secret masked, and closes", async () => {
    const onClose = vi.fn();
    render(<ManifestYamlDialog manifest={manifest} label="air" onClose={onClose} />);
    expect(screen.getByText(/a\.example/)).toBeInTheDocument();
    expect(screen.queryByText(/hunter2/)).toBeNull();
    expect(screen.getByText(/\*{8}/)).toBeInTheDocument();
    await userEvent.click(screen.getAllByRole("button", { name: "Close" })[0]!);
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe("workspaceMiddleware on a write", () => {
  afterEach(() => setActiveWorkspace(null));

  it("keeps the method and the body, and adds the copy to the address", async () => {
    setActiveWorkspace("air-v2");
    const out = (await workspaceMiddleware.onRequest!({
      request: new Request("http://localhost/api/v1/projects/helsinki/pipelines", {
        method: "POST",
        body: JSON.stringify({ name: "p" }),
      }),
    } as never)) as Request;
    expect(out.method).toBe("POST");
    expect(out.url).toContain("?workspace=air-v2");
    expect(await out.text()).toBe('{"name":"p"}');
  });
});
