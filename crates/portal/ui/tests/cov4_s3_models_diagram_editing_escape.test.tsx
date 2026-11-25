// T-7706: the new relationship dialog of the drawing closes on Escape and writes nothing.
import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { LinkmlGraphView } from "../src/pages/models/LinkmlGraphView";
import { renderPart } from "./page_contract";

const SCHOOL = `name: school
classes:
  School:
    slots: [title]
  Pupil:
    slots: [age]
slots:
  title:
    range: string
  age:
    range: integer
`;



const PEOPLE = `name: people
classes:
  Person:
    slots: [label]
slots:
  label:
    range: string
`;

/** The drawing over a source it holds itself, as the editor holds it; `written` sees every write. */
function Editable({ initial, written }: { initial: string; written?: (source: string) => void }) {
  const [source, setSource] = useState(initial);
  return (
    <>
      <LinkmlGraphView
        source={source}
        imports={{ people: PEOPLE }}
        onChange={(next) => {
          written?.(next);
          setSource(next);
        }}
      />
      <pre data-testid="source">{source}</pre>
    </>
  );
}

const sourceNow = () => screen.getByTestId("source").textContent ?? "";
const box = (name: string) => screen.getByRole("button", { name: en.models.graph.openClass.replace("{name}", name) });

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("the new relationship dialog", () => {
  it("closes on Escape and leaves the model as it was", async () => {
    const user = userEvent.setup();
    renderPart(<Editable initial={SCHOOL} />);
    fireEvent.pointerDown(screen.getByTestId("diagram-connect-School"));
    fireEvent.pointerUp(box("Pupil"));
    await screen.findByRole("dialog", { name: "New relationship from School" });
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(sourceNow()).toBe(SCHOOL);
  });
});
