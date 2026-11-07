/** T-2731: the ContextSourceRegistration create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "ContextSourceRegistration", address: "/projects/helsinki/csrs/new" });
