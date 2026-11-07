/** T-2731: the ContextSpace create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "ContextSpace", address: "/projects/helsinki/spaces/new" });
