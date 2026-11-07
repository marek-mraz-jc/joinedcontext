/** T-1547 x T-2731: the ModelProjection create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "ModelProjection", address: "/projects/helsinki/projections", button: "New model projection" });
