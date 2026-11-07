/**
 * T-2694: a question offers options a person may not take, disabled with the reason, and asks
 * for data: a file of theirs or a feed's address. The browser refuses what the Portal would, so a
 * wrong file is said here and nothing is sent; the answer is one of the three shapes of API/04 §5.
 */
import { render, screen } from "@testing-library/react";
import axe from "axe-core";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { QuestionOptions } from "../src/pages/apps/QuestionOptions";
import { QuestionData, inputOf } from "../src/pages/apps/QuestionData";
import type { QuestionInput } from "../src/pages/apps/QuestionData";
import { questionOf } from "../src/pages/apps/useAgentRun";
import type { JsonSchema } from "../src/components/forms/types";
import { xlsxFile } from "./xlsxFixture";

const SOURCES: JsonSchema = {
  type: "object",
  title: "Where does the data come from?",
  properties: {
    answer: {
      type: "string",
      title: "Where does the data come from?",
      oneOf: [
        { const: "datasource", title: "A data source this project already has" },
        { const: "space", title: "A context space of this project" },
      ],
      default: "space",
    },
  },
  required: ["answer"],
} as JsonSchema;

const REASON = "This project has no data source yet.";
const INPUT: QuestionInput = { file: { accept: ["csv", "tsv", "json"], maxBytes: 1024 }, url: true };

function options(onAnswer = vi.fn()) {
  render(
    <I18nextProvider i18n={i18n}>
      <QuestionOptions schema={SOURCES} disabledReasons={{ datasource: REASON }} onAnswer={onAnswer} />
    </I18nextProvider>,
  );
  return onAnswer;
}

function data(onAnswer = vi.fn(), input: QuestionInput = INPUT) {
  render(
    <I18nextProvider i18n={i18n}>
      <main>
        <QuestionData input={input} onAnswer={onAnswer} />
      </main>
    </I18nextProvider>,
  );
  return onAnswer;
}

const fileInput = () => screen.getByLabelText(en.agentRun.question.file.label);

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("an option a person cannot take", () => {
  it("stays in its place, disabled, with the reason read as its description", async () => {
    const onAnswer = options();
    const refused = screen.getByRole("radio", { name: /A data source this project already has/ });
    expect(refused).toHaveAttribute("aria-disabled", "true");
    expect(refused).toHaveAccessibleDescription(REASON);
    // The name is the option's, not the reason too: the reason is heard once.
    expect(refused).toHaveAccessibleName("A data source this project already has");
    await userEvent.setup().click(refused);
    expect(onAnswer).not.toHaveBeenCalled();
  });

  it("answers with the id of the option that is open", async () => {
    const onAnswer = options();
    await userEvent.setup().click(screen.getByRole("radio", { name: /A context space of this project/ }));
    expect(onAnswer).toHaveBeenCalledWith({ answer: "space" });
  });

  it("is reached by the arrows like the others, and Enter on it answers nothing", async () => {
    const onAnswer = options();
    const person = userEvent.setup();
    screen.getByRole("radio", { name: /A context space/ }).focus();
    await person.keyboard("{ArrowUp}");
    expect(document.activeElement).toBe(screen.getByRole("radio", { name: /A data source/ }));
    await person.keyboard("{Enter}");
    expect(onAnswer).not.toHaveBeenCalled();
    await person.keyboard("{ArrowDown}{Enter}");
    expect(onAnswer).toHaveBeenCalledWith({ answer: "space" });
  });
});

describe("data a question asks for", () => {
  it("sends a file of a format it takes as its name, format and text", async () => {
    const onAnswer = data();
    await userEvent.setup().upload(fileInput(), new File(["station,bikes\nkamppi,4\n"], "Stations.CSV", { type: "text/csv" }));
    expect(onAnswer).toHaveBeenCalledWith({
      file: { name: "Stations.CSV", format: "csv", text: "station,bikes\nkamppi,4\n" },
    });
  });

  it("refuses, before sending, a format it does not take, a file over its size, an empty one and broken JSON", async () => {
    const onAnswer = data();
    // The picker's own `accept` would hide the wrong file; a drop or a forced pick still arrives.
    const person = userEvent.setup({ applyAccept: false });
    const cases: [File, string][] = [
      [new File(["a"], "stations.xlsx"), i18n.t("agentRun.question.file.wrongType", { formats: "csv, tsv, json" })],
      [new File(["a".repeat(2048)], "big.csv"), i18n.t("agentRun.question.file.tooLarge", { kilobytes: 1 })],
      [new File([""], "empty.csv"), en.agentRun.question.file.empty],
      [new File(["{not json"], "broken.json"), en.agentRun.question.file.notJson],
    ];
    for (const [file, said] of cases) {
      await person.upload(fileInput(), file);
      expect(await screen.findByRole("alert")).toHaveTextContent(said);
    }
    expect(onAnswer).not.toHaveBeenCalled();
  });

  const EXCEL: QuestionInput = { file: { accept: ["csv", "tsv", "xlsx", "json"], maxBytes: 1024 } };

  it("sends a one-sheet Excel workbook as the CSV of its rows, dates as ISO dates (JC1-07)", async () => {
    const onAnswer = data(vi.fn(), EXCEL);
    expect(screen.getByText(/csv, tsv, xlsx, json/)).toBeInTheDocument();
    await userEvent.setup().upload(fileInput(), xlsxFile("stations.xlsx", { Sheet1: [["station", "opened"], ["kamppi", { date: 46086 }]] }));
    await vi.waitFor(() =>
      expect(onAnswer).toHaveBeenCalledWith({ file: { name: "stations.csv", format: "csv", text: "station,opened\r\nkamppi,2026-03-05\r\n" } }),
    );
  });

  it("asks which sheet of several before it sends one, and cuts a long sheet at a whole row", async () => {
    const onAnswer = data(vi.fn(), EXCEL);
    const person = userEvent.setup();
    const long = Array.from({ length: 200 }, (_, n) => [`stop-${n}`, n]);
    await person.upload(fileInput(), xlsxFile("city.xlsx", { Stations: [["station"], ["kamppi"]], Stops: [["code", "lines"], ...long] }));
    const sheet = await screen.findByRole("combobox", { name: "Sheet" });
    expect(onAnswer).not.toHaveBeenCalled();
    await person.selectOptions(sheet, "Stops");
    await person.click(screen.getByRole("button", { name: "Hand over the sheet Stops" }));
    const answer = onAnswer.mock.calls[0]?.[0] as { file: { name: string; format: string; text: string } };
    expect(answer.file.name).toBe("Stops.csv");
    expect(answer.file.format).toBe("csv");
    expect(new TextEncoder().encode(answer.file.text).length).toBeLessThanOrEqual(1024);
    expect(answer.file.text.startsWith("code,lines\r\nstop-0,0\r\n")).toBe(true);
    expect(answer.file.text.endsWith("\r\n")).toBe(true);
    const results = await axe.run(document.body);
    expect(results.violations.map((violation) => violation.id)).toEqual([]);
  });

  it("says a damaged workbook cannot be read and sends nothing", async () => {
    const onAnswer = data(vi.fn(), EXCEL);
    await userEvent.setup().upload(fileInput(), new File(["not a zip"], "broken.xlsx"));
    expect(await screen.findByRole("alert")).toHaveTextContent("not an Excel workbook");
    expect(onAnswer).not.toHaveBeenCalled();
  });

  it("sends a full http(s) address and refuses anything else with the reason on the field", async () => {
    const onAnswer = data();
    const person = userEvent.setup();
    const box = screen.getByLabelText(en.agentRun.question.url.label);
    for (const wrong of ["ftp://files.example.org/a.csv", "/relative/feed", "not an address"]) {
      await person.clear(box);
      await person.type(box, wrong);
      await person.click(screen.getByRole("button", { name: en.agentRun.question.url.use }));
      expect(box).toHaveAccessibleDescription(expect.stringContaining(en.agentRun.question.url.invalid));
    }
    expect(onAnswer).not.toHaveBeenCalled();
    await person.clear(box);
    await person.type(box, "https://api.citybik.es/v2/networks/citybikes-helsinki{Enter}");
    expect(onAnswer).toHaveBeenCalledWith({ url: "https://api.citybik.es/v2/networks/citybikes-helsinki" });
  });

  it("shows only what the question asks for, and has no accessibility violations", async () => {
    data(vi.fn(), { url: true });
    expect(screen.queryByLabelText(en.agentRun.question.file.label)).toBeNull();
    expect(screen.getByLabelText(en.agentRun.question.url.label)).toBeInTheDocument();
    const results = await axe.run(document.body);
    expect(results.violations.map((violation) => violation.id)).toEqual([]);
  });
});

describe("the question event", () => {
  it("carries the reasons and the input the Portal wrote, and nothing it did not", () => {
    const question = questionOf({
      seq: 3,
      kind: "question",
      payload: {
        questionId: "q-1",
        schema: SOURCES,
        options: [
          { value: "datasource", title: "A data source", disabledReason: REASON },
          { value: "space", title: "A context space" },
        ],
        input: { file: { accept: ["csv"], maxBytes: 262144 }, url: true },
      },
    });
    expect(question?.disabledReasons).toEqual({ datasource: REASON });
    expect(question?.input).toEqual({ file: { accept: ["csv"], maxBytes: 262144 }, url: true });
    expect(inputOf(null)).toBeUndefined();
    expect(inputOf({ file: { accept: [], maxBytes: 10 } })).toBeUndefined();
    expect(inputOf({ url: "yes" })).toBeUndefined();
  });
});
