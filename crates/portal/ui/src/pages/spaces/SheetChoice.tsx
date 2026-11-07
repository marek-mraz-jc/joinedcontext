/**
 * The sheet of a workbook an upload reads (JC1-07): offered only when the workbook holds more
 * than one, every sheet by the name the spreadsheet shows.
 */
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Field, Select } from "../../components/ui";
import type { Sheet } from "./importRows";

export function SheetChoice({ id, sheets, value, onChange }: { id: string; sheets: Sheet[]; value: string; onChange: (name: string) => void }): JSX.Element | null {
  const { t } = useTranslation();
  if (sheets.length < 2) return null;
  return (
    <Field id={id} className="max-w-md" label={t("spaces.sheet.label")} help={t("spaces.sheet.hint", { example: sheets[1]?.name ?? "" })}>
      <Select id={id} value={value} onChange={(e) => onChange(e.target.value)}>
        {sheets.map((sheet) => (
          <option key={sheet.name} value={sheet.name}>
            {t("spaces.sheet.option", { name: sheet.name, rows: sheet.table.rows.length, columns: sheet.table.columns.length })}
          </option>
        ))}
      </Select>
    </Field>
  );
}
