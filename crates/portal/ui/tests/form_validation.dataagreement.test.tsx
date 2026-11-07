/** T-1547 x T-2731: the DataAgreement create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "DataAgreement", address: "/projects/helsinki/dataagreements", button: "New data agreement" });
