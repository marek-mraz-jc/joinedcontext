/** T-2731: the Dashboard create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "Dashboard", address: "/projects/helsinki/dashboards", button: "New dashboard" });
