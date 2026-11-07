/** T-2731: the ServiceAccount create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "ServiceAccount", address: "/projects/helsinki/settings/service-accounts/new" });
