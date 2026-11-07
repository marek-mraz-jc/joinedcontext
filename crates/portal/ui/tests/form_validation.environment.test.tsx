/** T-1547 x T-2731: the Environment create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "Environment", address: "/organization/environments", button: "New environment" });
