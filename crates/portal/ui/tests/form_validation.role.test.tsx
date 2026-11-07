/** T-2731: the Role create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "Role", address: "/projects/helsinki/settings/roles/new" });
