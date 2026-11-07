import type { ComponentProps, ReactElement } from "react";
import { render, screen } from "@testing-library/react";
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from "@tanstack/react-router";
import { describe, expect, it } from "vitest";
import { OtherPageCrumb, SectionCrumb } from "../src/components/layout/Shell";

/** A crumb inside a router, as the shell renders it. */
async function inRouter(element: ReactElement, label: string) {
  render(
    <RouterProvider
      router={createRouter({
        routeTree: createRootRoute({ component: () => element }),
        history: createMemoryHistory({ initialEntries: ["/projects/helsinki/models/x"] }),
      })}
    />,
  );
  await screen.findByText(label);
}

async function crumb(model: string | undefined) {
  await inRouter(<OtherPageCrumb project="helsinki" label="Data models" model={model} />, "Data models");
}

async function section(props: Partial<ComponentProps<typeof SectionCrumb>>) {
  await inRouter(
    <SectionCrumb project="helsinki" plural="endpoints" label="Endpoints" detail={null} {...props} />,
    "Endpoints",
  );
}

describe("OtherPageCrumb", () => {
  it("names the page without a link outside a model", async () => {
    await crumb(undefined);
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.getByText("Data models")).toHaveAttribute("aria-current", "page");
  });

  it("links back to the models of a new model and names nothing else", async () => {
    await crumb("");
    expect(screen.getByRole("link", { name: "Data models" })).toHaveAttribute("href", "/projects/helsinki/models");
    expect(document.querySelector('span[aria-current="page"]')).toBeNull();
  });

  it("links back to the models and names the model", async () => {
    await crumb("air-quality");
    expect(screen.getByRole("link", { name: "Data models" })).toHaveAttribute("href", "/projects/helsinki/models");
    expect(screen.getByText("air-quality")).toHaveAttribute("aria-current", "page");
  });
});

describe("SectionCrumb", () => {
  it("names the list itself without a link", async () => {
    await section({});
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.getByText("Endpoints")).toHaveAttribute("aria-current", "page");
  });

  it("leads a create form back to its list", async () => {
    await section({ creating: true });
    expect(screen.getByRole("link", { name: "Endpoints" })).toHaveAttribute("href", "/projects/helsinki/endpoints");
    expect(document.querySelector('[aria-current="page"]')).toBeNull();
  });

  it("links the list and names the item of the address", async () => {
    await section({ itemName: "air-quality" });
    expect(screen.getByRole("link", { name: "Endpoints" })).toHaveAttribute("href", "/projects/helsinki/endpoints");
    expect(screen.getByRole("link", { name: "air-quality" })).toHaveAttribute(
      "href",
      "/projects/helsinki/endpoints/air-quality",
    );
  });

  it("names no item on a settings page", async () => {
    await section({ plural: "settings", itemName: "general" });
    expect(screen.queryByText("general")).toBeNull();
  });

  it("names a Context Spaces page in words beside its list", async () => {
    await section({ plural: "spaces", spacePage: "spaces.upload.title" });
    expect(screen.getByRole("link", { name: "Endpoints" })).toHaveAttribute("href", "/projects/helsinki/spaces");
    expect(document.querySelector('span[aria-current="page"]')).not.toBeNull();
  });

  it("leads an approval back to the approvals and names it", async () => {
    await section({ detail: { to: "/projects/$project/approvals", name: "ch-42" }, itemName: "ch-42" });
    expect(screen.getByRole("link", { name: "Endpoints" })).toHaveAttribute("href", "/projects/helsinki/approvals");
    expect(screen.getByText("ch-42")).toHaveAttribute("aria-current", "page");
  });
});
