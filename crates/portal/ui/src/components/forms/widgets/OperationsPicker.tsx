import { useState } from "react";
import type { JSX } from "react";
import { ariaDescribedByIds } from "@rjsf/utils";
import type { WidgetProps } from "@rjsf/utils";
import { useTranslation } from "react-i18next";
import {
  OPERATION_GROUPS,
  OPERATION_GROUP_NAMES,
  expandOperations,
  groupOf,
} from "../../endpoints/operationGroups";
import { Badge, Checkbox } from "../../ui";
import { useShownErrors } from "../touched";
import { AccessPresetChoice } from "./AccessPresetChoice";
import type { PresetMode } from "./AccessPresetChoice";
import { OPERATION_PRESETS, matchOperationsPreset } from "./accessPresets";

/**
 * What a `Policy` grants or a `ContextSourceRegistration` is registered for, picked by the five CIM 009 names first (R8, GW34, UI-01, T-2326).
 *
 * A grant is written in Table 4.20-2's group names, so the five groups are the whole of what a
 * person normally chooses between — and each one says, in the same breath, which operations it
 * stands for, because a name nobody can expand is a word nobody can check. The individual
 * operations are behind "more" and are the ones the groups are made of: the Portal names no
 * operation it cannot show the source of, and the API refuses any name outside Table 4.20-1
 * before it is stored.
 *
 * A group that changes context data is marked: `updateOps` and `redirectionOps` are not a
 * heavier shade of "read" (AP-09).
 */
export function OperationsPicker(props: WidgetProps): JSX.Element {
  const { id, value, disabled, readonly, onChange, rawErrors, options } = props;
  const { t } = useTranslation();
  const errors = useShownErrors(id, rawErrors);
  // What an empty choice means is the kind's to say: nothing on a policy, the specification's
  // default operations on a registration (T-2345).
  const none = typeof options.none === "string" ? options.none : t("policies.operations.none");
  const moreHint =
    typeof options.moreHint === "string" ? options.moreHint : t("policies.operations.moreHint");

  const chosen: string[] = Array.isArray(value) ? (value as string[]).map(String) : [];
  const described = ariaDescribedByIds(id);
  const frozen = Boolean(disabled || readonly);

  /**
   * The individual operations a person can pick, each once: what the five groups are made of,
   * plus anything this policy already grants that is none of them — a manifest written by hand
   * keeps what it says, and a person can see it and take it away.
   */
  const known = expandOperations(OPERATION_GROUP_NAMES);
  const single = [
    ...new Set([...known, ...chosen.filter((name) => !groupOf(name))]),
  ].sort((a, b) => a.localeCompare(b));
  /** Every operation the current choice covers, groups expanded (T-2282). */
  const covered = expandOperations(chosen);

  // `presets` (a service account grant, T-4832): a preset first, the groups only under Custom.
  const presets = options.presets === true;
  const [mode, setMode] = useState<PresetMode>(() => matchOperationsPreset(chosen) ?? "custom");
  const offered = Array.isArray(options.enumOptions)
    ? new Set(options.enumOptions.map((option) => String(option.value)))
    : undefined;

  const toggle = (name: string, on: boolean): void => {
    const next = on ? [...chosen, name] : chosen.filter((held) => held !== name);
    // The order a person ticked them in is not the order the manifest reads best: the groups
    // first, then the individual operations, so two policies written the same way look the same.
    const sorted = [
      ...OPERATION_GROUP_NAMES.filter((group) => next.includes(group)),
      ...next.filter((name) => !groupOf(name)).sort((a, b) => a.localeCompare(b)),
    ];
    onChange(sorted);
  };

  const picker = (
    <div
      role="group"
      aria-describedby={described}
      data-testid="operations-picker"
      className="flex flex-col gap-3"
    >
      {OPERATION_GROUP_NAMES.map((name) => {
        const group = OPERATION_GROUPS[name];
        const ticked = chosen.includes(name);
        return (
          <div key={name} className="flex flex-col gap-1">
            <Checkbox
              id={`${id}-${name}`}
              aria-describedby={described}
              checked={ticked}
              disabled={frozen}
              onChange={(event) => toggle(name, event.currentTarget.checked)}
              label={
                <span className="flex items-center gap-2">
                  {/* The group in words; its CIM 009 name is the manifest's, not the person's (T-2756). */}
                  <span>{t(`choice.operationGroup.${name}`)}</span>
                  {/* A space, so the box is named "… changes data", not "…changes data". */}
                  {group.writes ? " " : null}
                  {group.writes ? (
                    <Badge tone="warning">{t("policies.operations.writes")}</Badge>
                  ) : null}
                </span>
              }
            />
            <p className="pl-6 text-caption text-fg-muted">{t(`endpoints.page.group.${name}`)}</p>
            {/* The members, one tap away: a group is exactly the operations the table lists for
                it, and up to 22 identifiers in the body filled two phone screens (T-4211). */}
            <details className="pl-6 text-caption text-fg-muted">
              <summary className="cursor-pointer">
                {t("policies.operations.show", { count: group.operations.length })}
              </summary>
              <p className="mt-1 font-mono break-words">{group.operations.join(", ")}</p>
            </details>
          </div>
        );
      })}

      <details className="rounded-md border border-border p-3">
        <summary className="cursor-pointer text-body">
          {t("policies.operations.more")}
        </summary>
        <p className="mt-1 text-caption text-fg-muted">{moreHint}</p>
        <div className="mt-2 grid gap-1 sm:grid-cols-2">
          {single.map((name) => (
            <Checkbox
              key={name}
              id={`${id}-op-${name}`}
              aria-describedby={described}
              checked={chosen.includes(name)}
              disabled={frozen}
              onChange={(event) => toggle(name, event.currentTarget.checked)}
              label={<span className="font-mono text-caption">{name}</span>}
            />
          ))}
        </div>
      </details>

      <p className="text-caption text-fg-muted" data-testid="operations-summary">
        {chosen.length === 0
          ? none
          : t("policies.operations.total", { count: covered.length })}
      </p>
      {errors && errors.length > 0 ? (
        <p className="text-caption text-danger">{errors[0]}</p>
      ) : null}
    </div>
  );

  if (!presets) return picker;
  return (
    <AccessPresetChoice
      id={id}
      mode={mode}
      frozen={frozen}
      available={(preset) =>
        !offered || OPERATION_PRESETS[preset].every((name) => offered.has(name))
      }
      onPick={(next) => {
        setMode(next);
        if (next !== "custom") onChange([...OPERATION_PRESETS[next]]);
      }}
    >
      {picker}
    </AccessPresetChoice>
  );
}
