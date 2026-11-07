/** T-1547 x T-2731: the ScopeDefinition create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "ScopeDefinition", address: "/projects/helsinki/scopedefinitions", button: "New scope definition" });
