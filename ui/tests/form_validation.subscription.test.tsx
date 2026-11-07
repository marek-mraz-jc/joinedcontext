/** T-2731: the Subscription create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({
  kind: "Subscription",
  address: "/projects/helsinki/subscriptions/new",
  reference: { path: "spec.contextSpaceRef", field: "root_contextSpaceRef" },
});
