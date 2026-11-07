/** T-1547 x T-2731: the AgentProfile create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "AgentProfile", address: "/organization/agentprofiles", button: "New agent profile" });
