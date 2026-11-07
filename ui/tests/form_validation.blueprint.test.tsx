/** T-1547 x T-2731: the Blueprint create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "Blueprint", address: "/organization/blueprints", button: "New blueprint" });
