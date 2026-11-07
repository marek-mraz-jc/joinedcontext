import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/ui";
import { UnitPicker } from "../../components/pickers/UnitPicker";
import { conversion, convertible, unitOf } from "../../units";
import type { Unit } from "../../units";

/**
 * The Bloblang line that writes a slot from a source field in another unit (DM-06, T-2811): the
 * value converted with the code list's factor and offset, the `unitCode` the model's. Answers
 * undefined when the two units do not convert, so no line is ever offered for a temperature
 * into a concentration.
 */
export function conversionLine(slot: string, from: Unit, to: Unit): string | undefined {
  const linear = conversion(from, to);
  if (!linear) return undefined;
  const read = `this.${slot}.number()`;
  const scaled = linear.factor === 1 ? read : `${read} * ${linear.factor}`;
  const value =
    linear.offset === 0 ? scaled : `${scaled} ${linear.offset < 0 ? "-" : "+"} ${Math.abs(linear.offset)}`;
  return `root.${slot} = { "type": "Property", "value": ${value}, "unitCode": ${JSON.stringify(to.code)} }`;
}

/**
 * What a mapping needs to know about a slot's unit (DM-06, T-2811): the unit the model measures it
 * in, and, for a source that measures it in another, the line that converts it.
 */
export function SlotUnit({ slot, code, onAdd }: { slot: string; code: string; onAdd: (line: string) => void }): JSX.Element | null {
  const { t } = useTranslation();
  const [source, setSource] = useState("");
  const model = unitOf(code);
  if (!model) return null;
  const from = unitOf(source);
  const line = from ? conversionLine(slot, from, model) : undefined;
  const id = `slot-unit-${slot}`;
  return (
    <div className="flex flex-col gap-1">
      <p className="text-fg-muted">
        {t("pipelines.workbench.mapping.measuredIn", { symbol: model.symbol || model.name, code: model.code })}
      </p>
      <label htmlFor={id} className="text-fg-muted">
        {t("pipelines.workbench.mapping.sourceUnit")}
      </label>
      <UnitPicker
        id={id}
        label={t("pipelines.workbench.mapping.sourceUnitOf", { slot })}
        labelled
        value={source}
        onChange={setSource}
        among={(unit) => unit.code !== model.code && convertible(unit, model)}
      />
      {line ? (
        <div className="flex flex-wrap items-center gap-2">
          <code className="font-mono text-fg-subtle">{line}</code>
          <Button
            size="xs"
            variant="secondary"
            onClick={() => {
              onAdd(line);
            }}
          >
            {t("pipelines.workbench.mapping.addConversion")}
          </Button>
        </div>
      ) : null}
    </div>
  );
}
