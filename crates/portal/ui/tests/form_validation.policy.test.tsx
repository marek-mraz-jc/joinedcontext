/** T-2731: the Policy create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({
  kind: "Policy",
  address: "/projects/helsinki/policies/new",
  // T-3217: a space the server does not know is said on the space's own field.
  reference: { path: "spec.contextSpaceRef", field: "root_contextSpaceRef" },
});
