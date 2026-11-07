/** T-3057 x T-2731: the KnowledgeSource create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({
  kind: "KnowledgeSource",
  address: "/projects/helsinki/knowledgesources",
  button: "New knowledge source",
  lists: { "/ckaninstances": [{ apiVersion: "joinedcontext.com/v1alpha1", kind: "CkanInstance", metadata: { name: "open-data", namespace: "helsinki" }, spec: {} }] },
  reference: { path: "spec.ckanInstanceRef", field: "root_ckanInstanceRef" },
});
