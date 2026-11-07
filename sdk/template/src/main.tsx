import { startApp } from "@joinedcontext/sdk";
import "@joinedcontext/sdk/style.css";
import App from "./App";
import "./components/components.css";
import "./app.css";
import tokens from "./design-tokens.json";

startApp(App, { tokens });
