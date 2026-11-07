import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { shippedForms } from "../../schemas/forms";
import { portalThemeWidgets } from "./theme";
import { arrange, index, mergeUi, paths, prefilled } from "./uischema";
import { PORTAL_WIDGET_NAMES } from "./widgetNames";
import type { JsonSchema, UiSchema } from "./types";

export interface FormArrangementOptions {
  open: boolean;
  kind?: string;
  schema: JsonSchema;
  uiSchema?: UiSchema;
  lockedName?: string;
  project?: string;
  orgDomain?: string;
}

export interface FormArrangement {
  /** The manifest's arrangement merged over the caller's, the name read-only once it exists. */
  uiSchema: UiSchema | undefined;
  about?: string;
  guide?: string;
  /** Whether the kind's manifest marks any field advanced, so the switch is worth showing. */
  advancedFields: boolean;
  advanced: boolean;
  /** Shows or hides the advanced fields; `save` keeps the choice as the person's preference. */
  setAdvanced: (next: boolean, save: boolean) => void;
}

/**
 * How a kind's form is laid out (UI-02): the shipped `portal/forms/` manifests, the configuration
 * repository's over them, and the person's advanced-mode preference (CC-29).
 */
export function useFormArrangement(options: FormArrangementOptions): FormArrangement {
  const { open, kind, schema, uiSchema, lockedName, project, orgDomain } = options;
  const { t, i18n } = useTranslation();
  // One fetch for the whole of `portal/forms/`, shared by every dialog through the query cache.
  const forms = useQuery({
    queryKey: queryKeys.forms(),
    enabled: open && kind !== undefined,
    staleTime: 5 * 60_000,
    retry: false,
    queryFn: async () => unwrap(await api.GET("/api/v1/forms", {})),
  });
  // CC-29: the Git mechanics are on the form only when the person asked for them. The stored
  // preference starts it; the switch answers at once and saves in the background, so an
  // instance without a preferences database still has a working switch.
  const preferences = useQuery({
    queryKey: queryKeys.preferences(),
    enabled: open && kind !== undefined,
    retry: false,
    queryFn: async () => unwrap(await api.GET("/api/v1/preferences", {})),
  });
  const [advancedChoice, setAdvancedChoice] = useState<boolean | null>(null);
  const advanced = advancedChoice ?? preferences.data?.advancedMode === true;
  const saveAdvanced = useMutation({
    mutationFn: async (next: boolean) =>
      unwrap(await api.PUT("/api/v1/preferences", { body: { ...(preferences.data ?? {}), advancedMode: next } })),
  });

  // The fields the manifest may arrange, as one string: a page that rebuilds its schema object on
  // every render would otherwise hand the form a new arrangement on every keystroke, and RJSF
  // rebuilds the input the person is typing into (endpoint_form.test.tsx typed one character).
  const arrangeable = paths(schema).join("\u0000");
  // A required field its default fills may fold (T-4824).
  const required = (schema.required ?? []).filter((field) => !prefilled(schema.properties?.[field])).join("\u0000");
  const arranged = useMemo(() => {
    if (!kind) {
      return undefined;
    }
    // The arrangements the UI ships first, the configuration repository's after them: an
    // organization that writes help for one field keeps the shipped help on the others (UI-02).
    // The shipped ones arrange the form from the first paint, so the fields do not rearrange
    // themselves under the hands of someone who started typing at once.
    const indexed = index([...shippedForms, ...(forms.data?.items ?? [])]);
    const manifest = indexed.forms[kind];
    // What a manifest asked for and this form cannot do is worth saying, but not before the
    // request has answered: the organization's own manifest is not in hand yet, and a complaint
    // about a field it arranges would appear and disappear again.
    const answered = !forms.isPending;
    if (!manifest) {
      return { uiSchema: undefined, about: undefined, guide: undefined, problems: answered ? indexed.problems : [], advancedFields: false };
    }
    const result = arrange(manifest, {
      locale: i18n.language,
      properties: arrangeable ? arrangeable.split("\u0000") : [],
      required: required ? required.split("\u0000") : [],
      widgets: [...Object.keys(portalThemeWidgets), ...PORTAL_WIDGET_NAMES],
      advanced,
      // An example reads as this project's and this organization's own (T-2752).
      examples: { project, orgDomain },
    });
    return {
      uiSchema: result.uiSchema,
      about: result.about,
      guide: result.guide,
      problems: answered ? [...indexed.problems, ...result.problems] : [],
      advancedFields: Object.values(manifest.spec.fields ?? {}).some((field) => field.advanced === true),
    };
  }, [kind, forms.data, forms.isPending, arrangeable, required, i18n.language, advanced, project, orgDomain]);

  // The manifest arranges what it names; the caller's literal still covers what a manifest
  // cannot know, such as a name that is read-only once the resource exists.
  const lockedUiSchema = useMemo<UiSchema | undefined>(() => {
    const effective = arranged?.uiSchema ? (mergeUi(arranged.uiSchema, uiSchema) as UiSchema) : uiSchema;
    if (lockedName === undefined) {
      return effective;
    }
    const own = (effective as Record<string, unknown> | undefined)?.name;
    return {
      ...(effective as Record<string, unknown> | undefined),
      name: { ...(typeof own === "object" && own !== null ? own : {}), "ui:readonly": true },
    } as UiSchema;
  }, [arranged, uiSchema, lockedName]);

  // A manifest that asks for what the form cannot do is a defect for whoever wrote it, said in
  // the console, never to the person filling the form in (T-4165, photo audit T-4159).
  const problemsSaid = (arranged?.problems ?? []).join("\n");
  useEffect(() => {
    if (problemsSaid !== "") console.warn(`${kind}: ${t("form.uischemaProblems")}:\n${problemsSaid}`);
  }, [kind, problemsSaid, t]);

  return {
    uiSchema: lockedUiSchema,
    about: arranged?.about,
    guide: arranged?.guide,
    advancedFields: arranged?.advancedFields === true,
    advanced,
    setAdvanced: (next, save) => {
      setAdvancedChoice(next);
      if (save) saveAdvanced.mutate(next);
    },
  };
}
