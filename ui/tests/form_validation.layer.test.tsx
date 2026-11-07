/** T-2731: the Layer create form validates as the server does (see formValidation.tsx). */
import { formValidationSuite } from "./formValidation";

formValidationSuite({
  kind: "Layer",
  address: "/projects/helsinki/dashboards",
  button: "New layer",
});
