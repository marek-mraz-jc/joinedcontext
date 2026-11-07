import { useEffect, useState } from "react";
import type { JSX } from "react";
import type { FieldProps } from "@rjsf/utils";
import { AccessPresetChoice } from "./AccessPresetChoice";
import type { PresetMode } from "./AccessPresetChoice";
import { matchRolePreset, rolePresetRules } from "./accessPresets";

interface RulesSchema {
  items?: {
    properties?: {
      kinds?: { items?: { enum?: unknown[] } };
      verbs?: { items?: { oneOf?: { const?: unknown }[] } };
    };
  };
}

/** What the Role form offers: the author's own kinds and verbs, as `roleSchema` wrote them. */
export function offeredByRoleSchema(schema: unknown): { kinds: string[]; verbs: string[] } {
  const properties = (schema as RulesSchema).items?.properties;
  return {
    kinds: (properties?.kinds?.items?.enum ?? []).map(String),
    verbs: (properties?.verbs?.items?.oneOf ?? []).map((choice) => String(choice.const)),
  };
}

/**
 * A Role's `rules` picked as a preset (T-4832): Reader, Writer, Steward or Pipeline writer
 * writes the rules for them, cut to what the author holds; Custom shows the rule list itself.
 * Without a known list of kinds (free-text kinds) nothing can be checked, so only the list shows.
 */
export function RolePresetField(props: FieldProps): JSX.Element {
  const { ArrayField } = props.registry.fields;
  const { "ui:field": _field, ...rest } = props.uiSchema ?? {};
  const offered = offeredByRoleSchema(props.schema);
  const rules = Array.isArray(props.formData)
    ? (props.formData as { kinds?: string[]; verbs?: string[] }[])
    : undefined;
  // A new role holds nothing yet: it starts as a Reader, the narrowest preset.
  const blank = (rules ?? []).every(
    (rule) => (rule.kinds ?? []).length === 0 && (rule.verbs ?? []).length === 0,
  );
  const [mode, setMode] = useState<PresetMode>(
    () => matchRolePreset(rules, offered.kinds, offered.verbs) ?? (blank ? "reader" : "custom"),
  );
  const { onChange, fieldPathId } = props;
  const known = offered.kinds.length > 0;
  const write = (preset: PresetMode): void => {
    const given = preset === "custom" ? null : rolePresetRules(preset, offered.kinds, offered.verbs);
    if (given) onChange(given, fieldPathId.path);
  };
  const pick = (next: PresetMode): void => {
    setMode(next);
    write(next);
  };
  // A preset the author cannot give whole shows as Custom, so the rule list is never out of reach.
  const shown: PresetMode =
    mode !== "custom" && rolePresetRules(mode, offered.kinds, offered.verbs) === null ? "custom" : mode;
  // The author's rights arrive after the first render: write the starting preset only once they
  // are known, and only into a role nobody has filled in yet.
  const start = known && blank && mode !== "custom" ? rolePresetRules(mode, offered.kinds, offered.verbs) : null;
  const startKey = start ? JSON.stringify(start) : "";
  useEffect(() => {
    if (startKey) onChange(JSON.parse(startKey) as unknown, fieldPathId.path);
  }, [startKey, onChange, fieldPathId.path]);

  const list = <ArrayField {...props} uiSchema={rest} />;
  if (!known) return list;
  const frozen = Boolean(props.disabled || props.readonly);
  return (
    <AccessPresetChoice
      id={props.fieldPathId.$id}
      mode={shown}
      frozen={frozen}
      available={(preset) => rolePresetRules(preset, offered.kinds, offered.verbs) !== null}
      onPick={pick}
    >
      {list}
    </AccessPresetChoice>
  );
}
