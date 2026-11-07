/** T-2731: the DataSource create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "DataSource", address: "/projects/helsinki/datasources/new" });
