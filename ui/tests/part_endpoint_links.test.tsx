/**
 * T-1812, T-1815: the endpoint link pill and the export trigger (UI-15, UI-16, PF-50).
 *
 * `EndpointLink` is the one control that turns a value into a link a steward hands out, and no
 * unit test named it. The survey measured its `href` as built from a value; it goes through
 * `safeHref`, and these cases are what holds it there — a scheme that runs code has to lose its
 * link and keep its words, in a pill that is still reachable and still says where it goes.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import {
  ALL_REPRESENTATIONS,
  catalogueLinkOf,
  catalogueUrl,
  endpointUrl,
  EndpointLink,
  ENDPOINT_LINKS,
  hubUrl,
  mcpOrigin,
  REPRESENTATION_PATHS,
  representationUrl,
  servedRepresentations,
} from "../src/components/endpoints/links";
import { ExportButton } from "../src/components/export/ExportButton";
import { createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import { EndpointPage } from "../src/pages/endpoints/EndpointPage";

function wrap(node: React.ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("the endpoint link pill", () => {
  it("links_the_address_the_portal_built_and_opens_it_away_from_this_page_safely", () => {
    wrap(<EndpointLink href={endpointUrl("k7m2qz4tv6xh3n5jb2ryd3wcfa", "/ngsi-ld/v1/types")}>types</EndpointLink>);
    const link = screen.getByRole("link", { name: /types/ });
    expect(link).toHaveAttribute("href", `${window.location.origin}/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa/ngsi-ld/v1/types`);
    expect(link).toHaveAttribute("target", "_blank");
    // Without `rel`, the page it opens gets a handle on this one through `window.opener`.
    expect(link.getAttribute("rel")).toMatch(/noopener|noreferrer/);
    // The whole address as the tooltip: a pill is short, and a steward is about to paste it.
    expect(link).toHaveAttribute("title", expect.stringContaining("/api/endpoint/"));
  });

  it("keeps_the_words_and_withholds_the_link_for_a_scheme_that_could_run_code", () => {
    for (const href of ["javascript:alert(1)", "data:text/html,<script>1</script>", "vbscript:x"]) {
      const { unmount } = wrap(<EndpointLink href={href}>{href}</EndpointLink>);
      expect(screen.getByText(href)).toBeInTheDocument();
      expect(screen.queryByRole("link")).toBeNull();
      unmount();
    }
  });

  it("is_reached_by_the_keyboard_with_a_focus_ring_of_the_shared_kind", async () => {
    wrap(
      <>
        <EndpointLink href={endpointUrl("slug", "/file.csv")}>csv</EndpointLink>
        <EndpointLink muted href={endpointUrl("slug", "/access")}>
          access
        </EndpointLink>
      </>,
    );
    const links = screen.getAllByRole("link");
    for (const link of links) {
      expect(link).toHaveClass("focus-ring");
    }
    await userEvent.tab();
    expect(links[0]).toHaveFocus();
    await userEvent.tab();
    expect(links[1]).toHaveFocus();
  });

  it("every_representation_and_every_standing_link_is_a_path_under_the_endpoint", () => {
    // A path that escaped the endpoint's own prefix would be a link out of the slug's grants.
    for (const path of [...Object.values(REPRESENTATION_PATHS), ...ENDPOINT_LINKS.map((one) => one.path)]) {
      expect(path.startsWith("/"), path).toBe(true);
      expect(path).not.toContain("..");
      const url = new URL(endpointUrl("k7m2qz4tv6xh3n5jb2ryd3wcfa", path));
      expect(url.origin).toBe(window.location.origin);
      expect(url.pathname.startsWith("/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa"), path).toBe(true);
    }
  });

  // T-3019: an MCP client refuses resource metadata that names another URL than the one it called
  // (RFC 9728 §3.3), and the gateway names the data host api.{domain} (T-4632), so MCP addresses are handed out there.
  it("an_mcp_address_is_on_the_platform_host_and_every_other_on_this_origin", () => {
    const slug = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
    expect(representationUrl(slug, "mcp", "dev.city.example")).toBe(
      `https://api.dev.city.example/api/endpoint/${slug}/mcp`,
    );
    expect(representationUrl(slug, "csv", "dev.city.example")).toBe(
      `${window.location.origin}/api/endpoint/${slug}/file.csv`,
    );
    // T-4132: the WebSocket address is the one API/07 and the catalogue name, never the Portal's.
    expect(representationUrl(slug, "websocket", "dev.city.example")).toBe(
      `https://api.dev.city.example/api/endpoint/${slug}/ws`,
    );
    expect(hubUrl("dev.city.example")).toBe("https://api.dev.city.example/api/mcp");
    expect(representationUrl("a/../b", "mcp", "dev.city.example")).toBe(
      "https://api.dev.city.example/api/endpoint/a%2F..%2Fb/mcp",
    );
  });

  it("an_mcp_address_stays_on_this_origin_without_a_bare_platform_host", () => {
    expect(mcpOrigin("Dev.City.Example:8443")).toBe("https://api.dev.city.example:8443");
    for (const domain of [undefined, "", "  ", "evil.example/api", "user@evil.example", "https://evil.example", "a b"]) {
      expect(mcpOrigin(domain), String(domain)).toBe(window.location.origin);
    }
    expect(hubUrl(undefined)).toBe(`${window.location.origin}/api/mcp`);
  });

  it("a_catalogue_address_escapes_the_name_it_is_given", () => {
    expect(catalogueUrl("https://data.city.example", "air quality/../admin")).toBe(
      "https://data.city.example/dataset/air%20quality%2F..%2Fadmin",
    );
  });

  // T-3018: the address is the CkanInstance's own site, never `data.` put in front of the
  // Portal's host, and only an https site jc-core would accept becomes a link.
  it("a_catalogue_address_is_the_instance_site_and_https_only", () => {
    expect(catalogueUrl("https://data.city.example/", "air")).toBe("https://data.city.example/dataset/air");
    expect(catalogueUrl("https://city.example/open-data//", "air")).toBe(
      "https://city.example/open-data/dataset/air",
    );
    for (const refused of [
      "http://data.city.example",
      "javascript:alert(1)",
      "data.city.example",
      "https://data.city.example/?q=1",
      "https://data.city.example/#top",
      "",
    ]) {
      expect(catalogueUrl(refused, "air"), refused).toBeUndefined();
    }
  });

  it("a_catalogue_link_is_only_for_an_endpoint_that_publishes_and_whose_instance_is_known", () => {
    const sites = new Map([["banskabystrica/bb-open-data", "https://data.city.example"]]);
    const link = catalogueLinkOf(sites);
    const endpoint = (spec: Record<string, unknown>) => ({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "public-air", namespace: "banskabystrica" },
      spec,
    });
    const published = endpoint({ publish: { ckan: { instanceRef: { kind: "CkanInstance", name: "bb-open-data" } } } });
    expect(link("banskabystrica", published)).toBe("https://data.city.example/dataset/public-air");
    // The dataset name the Endpoint gives, when it gives one; a plain-string reference too.
    expect(
      link("banskabystrica", endpoint({ publish: { ckan: { instanceRef: "bb-open-data", name: "ovzdusie" } } })),
    ).toBe("https://data.city.example/dataset/ovzdusie");
    expect(link("banskabystrica", endpoint({}))).toBeUndefined();
    // Another project's instance of the same name is not this one.
    expect(link("helsinki", published)).toBeUndefined();
    expect(
      link("banskabystrica", endpoint({ publish: { ckan: { instanceRef: { name: "unknown" } } } })),
    ).toBeUndefined();
  });
});

describe("what an endpoint serves", () => {
  // EP-10, EP-24, T-2901: NGSI-LD and MCP are on for every Endpoint, public or internal, without
  // being listed.
  it("adds_ngsi_ld_and_mcp_to_every_endpoint", () => {
    expect(servedRepresentations({ enabledRepresentations: ["ngsi-ld", "csv"] })).toEqual(["ngsi-ld", "csv", "mcp"]);
    expect(servedRepresentations({ enabledRepresentations: ["mcp", "ngsi-ld"] })).toEqual(["mcp", "ngsi-ld"]);
    // No list (EP-104): every standard but the opt-in sta and ical (EP-125), less the disabled ones.
    const everyButSta = ALL_REPRESENTATIONS.filter((rep) => rep !== "sta" && rep !== "ical");
    expect(servedRepresentations({})).toEqual(everyButSta);
    expect(servedRepresentations({ disabledRepresentations: ["csv"] })).toEqual(
      everyButSta.filter((rep) => rep !== "csv"),
    );
  });

  it("leaves_mcp_out_only_when_the_manifest_says_mcp_false", () => {
    expect(servedRepresentations({ enabledRepresentations: ["csv"], mcp: false })).toEqual(["csv", "ngsi-ld"]);
    // Anything but the boolean false is not an opt-out: the gateway would serve the instance.
    expect(servedRepresentations({ enabledRepresentations: ["ngsi-ld"], mcp: "false" })).toEqual(["ngsi-ld", "mcp"]);
  });

  // T-4600: CSV is the one file an Endpoint offers. A manifest the migration has not reached yet
  // still names a retired one; no link to it is shown, because the gateway refuses it.
  it("offers_csv_as_the_only_file", () => {
    expect(
      servedRepresentations({ enabledRepresentations: ["ngsi-ld", "geojson", "csv", "xlsx", "parquet", "json", "zip"] }),
    ).toEqual(["ngsi-ld", "csv", "mcp"]);
    const files = (reps: string[]) => reps.filter((rep) => REPRESENTATION_PATHS[rep]?.startsWith("/file."));
    expect(files(servedRepresentations({}))).toEqual(["csv"]);
    expect(files(Object.keys(REPRESENTATION_PATHS))).toEqual(["csv"]);
  });

  it("drops_a_list_entry_that_is_not_a_name", () => {
    expect(servedRepresentations({ enabledRepresentations: ["ngsi-ld", 7, null], mcp: false })).toEqual(["ngsi-ld"]);
    // Not a list at all reads as no list: every standard but the opt-in sta and ical.
    expect(servedRepresentations({ enabledRepresentations: "ngsi-ld" })).toEqual(
      ALL_REPRESENTATIONS.filter((rep) => rep !== "sta" && rep !== "ical"),
    );
  });
});

describe("the export trigger", () => {
  it("opens_the_dialog_from_the_keyboard_and_closes_it_back_onto_its_button", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() =>
        Promise.resolve(
          new Response(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        ),
      ),
    );
    wrap(<ExportButton project="helsinki" target={{}} label={en.organization.projects.export} />);

    const trigger = screen.getByRole("button", { name: en.organization.projects.export });
    trigger.focus();
    await userEvent.keyboard("{Enter}");

    const dialog = await screen.findByRole("dialog");
    // The shared Dialog, which is what carries the focus trap and the escape key: a hand-built
    // Radix dialog had none of it (T-1816).
    expect(within(dialog).getAllByRole("radio")).toHaveLength(3);
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(trigger).toHaveFocus();
  });

  it("renders_no_trigger_when_the_row_menu_owns_it_and_still_opens_on_the_rows_word", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() =>
        Promise.resolve(
          new Response(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        ),
      ),
    );
    const { rerender } = wrap(
      <ExportButton project="helsinki" target={{}} label={en.organization.projects.export} trigger={false} open={false} />,
    );
    expect(screen.queryByRole("button", { name: en.organization.projects.export })).toBeNull();
    expect(screen.queryByRole("dialog")).toBeNull();

    rerender(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <ExportButton project="helsinki" target={{}} label={en.organization.projects.export} trigger={false} open />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });
});

/**
 * T-3018: the Endpoint page's Publication section links the dataset on its CkanInstance's own
 * site, and names it without a link while that site is unknown, never `data.` + the Portal's host.
 */
describe("the endpoint page's catalogue entry", () => {
  const PROJECT = "banskabystrica";
  const NAME = "public-air";
  const endpoint = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: NAME, namespace: PROJECT },
    spec: {
      contextSpaceRef: "ovzdusie",
      slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
      audience: "public",
      enabledRepresentations: ["ngsi-ld"],
      publish: { ckan: { instanceRef: { kind: "CkanInstance", name: "bb-open-data" }, name: "ovzdusie-bb" } },
    },
  };
  const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

  function show(catalogues: { status: number; items?: unknown[] }) {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const path = new URL(input instanceof Request ? input.url : String(input), window.location.origin).pathname;
        const json = (body: unknown, status = 200) =>
          Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }));
        if (path.endsWith(`/endpoints/${NAME}`)) return json(endpoint);
        if (path.endsWith("/ckaninstances")) {
          return catalogues.status === 200
            ? json(list(catalogues.items ?? []))
            : json({ title: "Forbidden", status: catalogues.status }, catalogues.status);
        }
        if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: false, grants: [] });
        return json(list([]));
      }),
    );
    const rootRoute = createRootRoute({ component: () => <EndpointPage project={PROJECT} name={NAME} /> });
    const router = createRouter({ routeTree: rootRoute });
    wrap(<RouterProvider router={router} />);
  }

  it("links the dataset on the site of the catalogue the endpoint names", async () => {
    show({
      status: 200,
      items: [
        {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "CkanInstance",
          metadata: { name: "bb-open-data", namespace: PROJECT },
          spec: { url: "https://data.city.example", secretRef: { name: "ckan-token" } },
        },
      ],
    });
    expect(await screen.findByRole("link", { name: "ovzdusie-bb" })).toHaveAttribute(
      "href",
      "https://data.city.example/dataset/ovzdusie-bb",
    );
  });

  it("names the dataset without a link when the catalogue cannot be read", async () => {
    show({ status: 403 });
    expect(await screen.findByText("ovzdusie-bb")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "ovzdusie-bb" })).not.toBeInTheDocument();
  });
});
