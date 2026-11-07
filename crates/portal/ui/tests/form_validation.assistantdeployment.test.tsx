/** T-3057 x T-2731: the AssistantDeployment create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({
  kind: "AssistantDeployment",
  address: "/projects/helsinki/assistantdeployments",
  button: "New assistant deployment",
  reference: { path: "spec.publicId", field: "root_publicId" },
});
