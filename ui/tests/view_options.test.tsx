/** What a view hides, colours and groups by (API/01 §30, T-3099). */
import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { EntitySource, GridQuery } from "@joinedcontext/sdk";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { GroupCounts, groupTerm, ViewOptions } from "../src/components/entities/ViewOptions";
import type { ViewExtras } from "../src/components/entities/ViewOptions";

const L = en.spaces.saved;
const STATUS = [
  { value: "working", title: "Working" },
  { value: "outOfService", title: "Out of service" },
];

function wrap(node: React.ReactNode) {
  return render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{node}</QueryClientProvider>
    </I18nextProvider>,
  );
}

function Harness({ onChange }: { onChange: (next: ViewExtras) => void }) {
  const [value, setValue] = useState<ViewExtras>({});
  return (
    <ViewOptions
      attributes={["availableBikeNumber", "name", "status"]}
      enums={{ status: STATUS }}
      value={value}
      onChange={(next) => {
        setValue(next);
        onChange(next);
      }}
    />
  );
}

describe("ViewOptions", () => {
  it("hides a field, adds a colour rule and groups by an enum attribute", async () => {
    await i18n.changeLanguage("en");
    const onChange = vi.fn();
    wrap(<Harness onChange={onChange} />);
    await userEvent.click(screen.getByText(L.options));

    await userEvent.click(screen.getByRole("checkbox", { name: "name" }));
    expect(onChange).toHaveBeenLastCalledWith({ hidden: ["name"] });

    await userEvent.click(screen.getByRole("button", { name: L.addRule }));
    await userEvent.type(screen.getByLabelText(L.when), "availableBikeNumber==0");
    await userEvent.selectOptions(screen.getByLabelText(L.tone), "danger");
    expect(onChange).toHaveBeenLastCalledWith({ hidden: ["name"], colour: [{ when: "availableBikeNumber==0", colour: "danger" }] });

    await userEvent.selectOptions(screen.getByLabelText(L.group), "status");
    expect(onChange).toHaveBeenLastCalledWith(expect.objectContaining({ group: "status" }));
  });

  it("says when a rule cannot be checked on the page", async () => {
    await i18n.changeLanguage("en");
    wrap(<Harness onChange={() => {}} />);
    await userEvent.click(screen.getByText(L.options));
    await userEvent.click(screen.getByRole("button", { name: L.addRule }));
    await userEvent.type(screen.getByLabelText(L.when), "(a==1|b==2);c==3");
    expect(screen.getByLabelText(L.when)).toHaveAccessibleDescription(expect.stringContaining(L.whenUnreadable));
  });
});

describe("GroupCounts", () => {
  it("counts each group with the space's own count, narrowed by the view's query", async () => {
    await i18n.changeLanguage("en");
    const asked: GridQuery[] = [];
    const source: EntitySource = {
      query: async (q) => {
        asked.push(q);
        if (q.q?.includes("outOfService")) throw new Error("refused");
        return { rows: [], total: 12 };
      },
      get: async () => null,
    };
    const onChoose = vi.fn();
    wrap(<GroupCounts scope="helsinki/bikes" source={source} type="BikeHireDockingStation" attr="status" options={STATUS} q="availableBikeNumber<3" chosen={null} onChoose={onChoose} />);

    const groups = screen.getByRole("navigation", { name: L.groupsOf.replace("{attr}", "status") });
    expect(await within(groups).findByRole("button", { name: "Working (12)" })).toBeInTheDocument();
    // A count the space would not give is said, never a zero.
    expect(await within(groups).findByRole("button", { name: `Out of service (${L.countUnknown})` })).toBeInTheDocument();
    expect(asked.map((q) => q.q)).toContain(`availableBikeNumber<3;${groupTerm("status", "working")}`);

    await userEvent.click(within(groups).getByRole("button", { name: "Working (12)" }));
    await waitFor(() => expect(onChoose).toHaveBeenCalledWith("working"));
    expect(within(groups).getByRole("button", { name: L.allGroups })).toHaveAttribute("aria-pressed", "true");
  });
});
