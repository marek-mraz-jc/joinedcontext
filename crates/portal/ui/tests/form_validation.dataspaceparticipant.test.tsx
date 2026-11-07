/** T-1547 x T-2731: the DataSpaceParticipant create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "DataSpaceParticipant", address: "/organization/dataspaceparticipants", button: "New data space membership" });
