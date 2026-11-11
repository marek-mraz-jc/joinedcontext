/**
 * The names a uiSchema may give `ui:widget` for the Portal's own widgets, without the widgets: a
 * form arranges itself by name before its editors load (T-3316). `portalWidgets` holds the same
 * keys; tests/widget_names.test.ts keeps the two in step.
 */
export const PORTAL_WIDGET_NAMES = [
  "assigneePicker",
  "attributeSuggest",
  "colour",
  "cronSchedule",
  "dataModelPicker",
  "entityPicker",
  "operations",
  "personPicker",
  "resourcePicker",
  "secretRef",
  "svgImage",
  "typePicker",
] as const;
