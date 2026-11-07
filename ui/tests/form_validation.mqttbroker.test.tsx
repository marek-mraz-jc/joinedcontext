/** T-2731: the MqttBroker create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({ kind: "MqttBroker", address: "/projects/helsinki/mqttbrokers/new" });
