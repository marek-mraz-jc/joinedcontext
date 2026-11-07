/** T-2731: the Group create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "Group", address: "/organization/groups/new" });
