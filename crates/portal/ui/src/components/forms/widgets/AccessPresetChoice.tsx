import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { RadioGroup } from "../../ui/RadioGroup";
import { ACCESS_PRESETS } from "./accessPresets";
import type { AccessPreset } from "./accessPresets";

export type PresetMode = AccessPreset | "custom";

/**
 * Reader, Writer, Steward, Pipeline writer or Custom as one radio group (T-4832); the raw list a
 * preset stands for is `children`, shown under "Advanced" only once Custom is chosen.
 */
export function AccessPresetChoice({
  id,
  mode,
  available,
  frozen,
  onPick,
  children,
}: {
  id: string;
  mode: PresetMode;
  available: (preset: AccessPreset) => boolean;
  frozen: boolean;
  onPick: (mode: PresetMode) => void;
  children: ReactNode;
}): JSX.Element {
  const { t } = useTranslation();
  // A preset the author cannot give is shown, disabled, with why as its own description.
  const options = [
    ...ACCESS_PRESETS.map((preset) => ({
      value: preset,
      label: t(`access.presets.${preset}.label`),
      description: available(preset)
        ? t(`access.presets.${preset}.description`)
        : t("access.presets.unavailable"),
      disabled: frozen || !available(preset),
    })),
    {
      value: "custom" as const,
      label: t("access.presets.custom.label"),
      description: t("access.presets.custom.description"),
      disabled: frozen,
    },
  ];
  return (
    <div className="flex flex-col gap-3" data-testid="access-preset">
      <RadioGroup<PresetMode>
        name={`${id}-preset`}
        legend={t("access.presets.legend")}
        value={mode}
        onChange={onPick}
        options={options}
      />
      {mode === "custom" ? (
        <section
          aria-labelledby={`${id}-advanced`}
          className="flex flex-col gap-2 rounded-md border border-border p-3"
        >
          <h4 id={`${id}-advanced`} className="text-body font-medium">
            {t("access.presets.advanced")}
          </h4>
          {children}
        </section>
      ) : null}
    </div>
  );
}
