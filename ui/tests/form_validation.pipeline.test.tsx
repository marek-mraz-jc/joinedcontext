/** T-2731: the Pipeline create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "Pipeline", address: "/projects/helsinki/pipelines/new" });
