/** T-2731: the SyncSource create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "SyncSource", address: "/projects/helsinki/syncsources/new" });
