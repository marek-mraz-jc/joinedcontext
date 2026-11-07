/** T-2731: the Endpoint create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({
  kind: "Endpoint",
  address: "/projects/helsinki/endpoints/new",
  reference: { path: "spec.contextSpaceRef", field: "root_contextSpaceRef" },
});
