/**
 * T-1732: what the browser is allowed to do with what the server sent (UI-39, UI-45, UI-46).
 *
 * The Portal frames applications, follows URLs out of manifests and run records, and keeps
 * working state in the browser of whichever machine somebody signed in from. Each of these is a
 * place where data becomes behaviour, and each of these tests names one of them.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/components/ui/SourceLink.tsx, src/components/ui/safeHref.ts through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { ErrorBoundary } from "../src/components/ErrorBoundary";
import { EndpointLink } from "../src/components/endpoints/links";
import { SourceLink } from "../src/components/ui";
import { clearBrowserState } from "../src/auth/browserState";
import { previewSrc } from "../src/pages/apps/previewBridge";
import { appAddress, appFrameSandbox } from "../src/pages/apps/AppOpenPage";

const ui = resolve(dirname(fileURLToPath(import.meta.url)), "..");

describe("a_preview_frame_cannot_reach_the_portal_origin", () => {
  it("only a path under /apps/ on this origin may be framed", () => {
    for (const good of [
      "/apps/air-quality/",
      "/apps/a/?preview=abc123",
      "/apps/a%2Fb/",
      "/api/v1/projects/helsinki/agent-runs/run-1/preview?v=3",
    ]) {
      expect(previewSrc(good), good).toBe(good);
    }
    for (const bad of [
      "javascript:alert(1)",
      "data:text/html,<script>alert(1)</script>",
      "https://evil.example/apps/x/",
      // An authority hidden behind what looks like a path; browsers read both as a host.
      "//evil.example/apps/x/",
      "/apps/../admin",
      "/api/v1/projects",
      // The run's own shape, but pointed at something else under the same prefix.
      "/api/v1/projects/helsinki/agent-runs/run-1/files",
      "",
      undefined,
      null,
      42,
    ]) {
      expect(previewSrc(bad), String(bad)).toBeUndefined();
    }
  });

  it("no frame in the Portal joins allow-scripts to allow-same-origin", () => {
    // The pair on a same-origin frame is not a sandbox: the framed document reaches the CSRF
    // cookie and writes as the signed-in person (AP-19). Read from the sources, because the rule
    // has to hold for the frames nobody wrote a render test for.
    const frames: string[] = [];
    const walk = (dir: string) => {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const full = join(dir, entry.name);
        if (entry.isDirectory()) walk(full);
        else if (/\.tsx$/.test(entry.name)) {
          const text = readFileSync(full, "utf8");
          for (const tag of text.matchAll(/<iframe\b[\s\S]*?\/>/g)) {
            const line = `${relative(ui, full)}:${text.slice(0, tag.index).split("\n").length}`;
            const sandbox = /sandbox="([^"]*)"/.exec(tag[0])?.[1];
            // The one sandbox that is not a literal: `appFrameSandbox` adds allow-same-origin only
            // for a frame on another origin than the Portal's, which its own tests below hold.
            const decided = /sandbox=\{appFrameSandbox\(/.test(tag[0]);
            if (sandbox === undefined && !decided) frames.push(`${line} has no sandbox`);
            else if (sandbox === undefined) continue;
            else if (sandbox.includes("allow-scripts") && sandbox.includes("allow-same-origin"))
              frames.push(`${line} joins allow-scripts to allow-same-origin`);
            if (!/\btitle=/.test(tag[0])) frames.push(`${line} has no title`);
            if (!/referrerPolicy="no-referrer"/.test(tag[0]))
              frames.push(`${line} sends a referrer`);
          }
        }
      }
    };
    walk(join(ui, "src"));
    expect(frames).toEqual([]);
  });
});

describe("the App's frame keeps an origin only when it is not the Portal's (AP-122, AP-19, T-2840)", () => {
  const portal = "https://portal.dev.example.org";

  it("never joins allow-same-origin for a document on the Portal's own origin", () => {
    for (const src of [
      "/apps/air/",
      "apps/air/",
      "https://portal.dev.example.org/apps/air/",
      "//portal.dev.example.org/apps/air/",
      "data:text/html,<script>1</script>",
      "javascript:alert(1)",
      "about:blank",
      "http://[bad",
    ]) {
      expect(appFrameSandbox(src, portal), src).not.toContain("allow-same-origin");
      expect(appFrameSandbox(src, portal), src).toContain("allow-scripts");
    }
  });

  it("gives an App on the apps origin its own origin, and nothing more", () => {
    const sandbox = appFrameSandbox("https://air.apps.dev.example.org/", portal).split(" ");
    expect(sandbox.sort()).toEqual(
      ["allow-downloads", "allow-forms", "allow-popups", "allow-same-origin", "allow-scripts"],
    );
  });

  it("frames the App on its own host under the apps origin the Portal names, or on its own path (AP-133)", () => {
    expect(appAddress("air", "https://dev.example.org")).toBe("https://air.apps.dev.example.org/");
    expect(appAddress("air", "https://dev.example.org:8443/ignored")).toBe("https://air.apps.dev.example.org:8443/");
    // A name no host could carry, or one that would climb to another host, keeps the path.
    for (const name of ["air quality", "evil.example.net#", "a.b", "-air", "Air", ""]) {
      expect(appAddress(name, "https://dev.example.org"), name).toBe(`/apps/${encodeURIComponent(name)}/`);
    }
    expect(appAddress("air", null)).toBe("/apps/air/");
    expect(appAddress("air", "javascript:alert(1)")).toBe("/apps/air/");
    expect(appAddress("air", "not a url")).toBe("/apps/air/");
  });
});

describe("sign_out_clears_storage", () => {
  beforeEach(() => {
    localStorage.clear();
    sessionStorage.clear();
  });

  it("leaving takes the projects, endpoints and runs of the person who was here", () => {
    sessionStorage.setItem("jc.assistant.run", "run-42");
    sessionStorage.setItem("jc.assistant.endpoints.helsinki", '["ep-bikes"]');
    localStorage.setItem("jc.grid.helsinki.AirQualityObserved", '{"columns":[]}');
    localStorage.setItem("jc-lang", "cs");
    localStorage.setItem("jc-theme", "dark");

    clearBrowserState();

    expect(sessionStorage.length).toBe(0);
    expect(localStorage.getItem("jc.grid.helsinki.AirQualityObserved")).toBeNull();
    // How the machine is set up, not who was using it: the sign-in page comes back in Czech.
    expect(localStorage.getItem("jc-lang")).toBe("cs");
    expect(localStorage.getItem("jc-theme")).toBe("dark");
  });

  it("storage the browser refuses is not an error", () => {
    const blocked = {
      sessionStorage: {
        clear() {
          throw new DOMException("denied", "SecurityError");
        },
      },
    } as unknown as Window;
    expect(() => clearBrowserState(blocked)).not.toThrow();
  });
});

describe("an_error_shows_no_stack_and_no_token", () => {
  const Boom = (): never => {
    throw new Error("GET /api/v1/projects?access_token=s3cr3t failed at line 12 of index-abc.js");
  };
  let logged: unknown[][] = [];

  beforeEach(() => {
    logged = [];
    vi.spyOn(console, "error").mockImplementation((...args: unknown[]) => {
      logged.push(args);
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("the page says what happened and carries nothing out of the error", () => {
    render(
      <ErrorBoundary>
        <Boom />
      </ErrorBoundary>,
    );

    const page = screen.getByRole("alert");
    expect(page).toHaveTextContent("This page stopped");
    expect(page.textContent).not.toMatch(/s3cr3t|access_token|index-abc\.js|line 12|Error:/);
    // A reference to quote, and the two things that help.
    expect(screen.getByTestId("error-reference").textContent).toMatch(/^[0-9a-z-]{4,}$/);
    expect(screen.getByRole("button", { name: "Load the page again" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Go to the start" })).toHaveAttribute("href", "/");
  });

  it("the same reference is in the console line a developer reads", () => {
    render(
      <ErrorBoundary>
        <Boom />
      </ErrorBoundary>,
    );
    const shown = screen.getByTestId("error-reference").textContent;
    expect(logged.some((args) => String(args[0]).includes(String(shown)))).toBe(true);
  });

  it("a tree that renders is left alone", () => {
    render(
      <ErrorBoundary>
        <p>The endpoints of helsinki</p>
      </ErrorBoundary>,
    );
    expect(screen.getByText("The endpoints of helsinki")).toBeInTheDocument();
    expect(screen.queryByTestId("error-reference")).not.toBeInTheDocument();
  });
});

describe("a_javascript_url_from_a_manifest_is_text", () => {
  const wrap = (node: React.ReactNode) =>
    render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);

  it("the forge icon of a manifest that names a javascript URL is not a link", () => {
    // `status.sourceUrl` is written by whoever authored the manifest, and five pages draw it.
    wrap(<SourceLink href="javascript:fetch('/api/v1/projects')" label="Source" />);
    expect(screen.queryByRole("link")).not.toBeInTheDocument();

    wrap(<SourceLink href="https://gitea.example/org/repo" label="Source" />);
    expect(screen.getByRole("link", { name: "Source" })).toHaveAttribute(
      "href",
      "https://gitea.example/org/repo",
    );
  });

  it("an endpoint pill with an address the Portal did not build keeps its words, not its link", () => {
    wrap(<EndpointLink href="javascript:alert(1)">ngsi-ld</EndpointLink>);
    expect(screen.getByText("ngsi-ld")).toBeInTheDocument();
    expect(screen.queryByRole("link")).not.toBeInTheDocument();

    wrap(<EndpointLink href="/api/endpoint/abc/ngsi-ld/v1/entities">geojson</EndpointLink>);
    expect(screen.getByRole("link", { name: /geojson/ })).toHaveAttribute(
      "href",
      "/api/endpoint/abc/ngsi-ld/v1/entities",
    );
  });

  it("no page hands a raw value straight to an anchor", () => {
    // What an `href={…}` may be: a template the page builds from encoded parts, a call to one of
    // the URL builders that only ever return a path on this host, or a binding a guard returned.
    // `lines.vscode` and `lines.cursor` are mcpInstall()'s editor deep links: fixed schemes and
    // every part URI-encoded (T-4114).
    const checked = /^`|^safeHref\(|^catalogueUrl\(|^endpointUrl\(|^(?:safe|preview)$|^lines\.(?:vscode|cursor)$/;
    const raw: string[] = [];
    const walk = (dir: string) => {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const full = join(dir, entry.name);
        if (entry.isDirectory()) walk(full);
        else if (/\.tsx$/.test(entry.name) && !full.includes("/components/ui/")) {
          const text = readFileSync(full, "utf8");
          for (const tag of text.matchAll(/<a\b[\s\S]*?>/g)) {
            const href = /href=\{([^}]*)\}/.exec(tag[0])?.[1];
            if (href === undefined || checked.test(href.trim())) continue;
            raw.push(`${relative(ui, full)}:${text.slice(0, tag.index).split("\n").length} ${href}`);
          }
        }
      }
    };
    walk(join(ui, "src"));
    expect(raw).toEqual([]);
  });
});
