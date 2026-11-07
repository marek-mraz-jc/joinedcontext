/** T-1547 x T-2731: the DataOffer create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "DataOffer", address: "/projects/helsinki/dataoffers", button: "New data offer" });
