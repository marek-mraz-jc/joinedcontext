import { startApp } from "@joinedcontext/sdk";
import "@joinedcontext/sdk/style.css";
import App from "./App";
import "./components/components.css";
import "./app.css";
import { startTokens } from "./theme";

startApp(App, { tokens: startTokens() });
