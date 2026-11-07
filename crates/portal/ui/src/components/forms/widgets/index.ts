import { AttributeCapabilitiesField } from "./AttributeCapabilitiesField";
import { EntityPicker } from "./EntityPicker";
import { EntitySelectorField, FormDataContext } from "./EntitySelectorField";
import { RepresentationOutputsField } from "./RepresentationOutputsField";
import { KnowledgeSourcesField } from "./KnowledgeSourcesField";
import { DataModelPickerWidget, FormProjectContext, TypePickerWidget } from "./ModelWidgets";
import { OperationsPicker } from "./OperationsPicker";
import { ResourcePicker } from "./ResourcePicker";
import { RolePresetField } from "./RolePresetField";
import { SecretRefWidget } from "./SecretRef";
import { AssigneePicker, AttributeSuggest, PersonPickerWidget } from "./SuggestWidgets";
import { CronScheduleWidget } from "./CronSchedule";

export {
  DataModelPickerWidget,
  EntityPicker,
  EntitySelectorField,
  FormDataContext,
  FormProjectContext,
  OperationsPicker,
  ResourcePicker,
  SecretRefWidget,
  TypePickerWidget,
};
export const portalWidgets = {
  assigneePicker: AssigneePicker,
  attributeSuggest: AttributeSuggest,
  cronSchedule: CronScheduleWidget,
  dataModelPicker: DataModelPickerWidget,
  entityPicker: EntityPicker,
  operations: OperationsPicker,
  personPicker: PersonPickerWidget,
  resourcePicker: ResourcePicker,
  secretRef: SecretRefWidget,
  typePicker: TypePickerWidget,
};

/** The Portal's own fields, which a uiSchema names with `ui:field`. */
export const portalFields = {
  attributeCapabilities: AttributeCapabilitiesField,
  entitySelector: EntitySelectorField,
  representationOutputs: RepresentationOutputsField,
  knowledgeSources: KnowledgeSourcesField,
  rolePreset: RolePresetField,
};
