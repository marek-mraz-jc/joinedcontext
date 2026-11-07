import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { apiBase, notesApi } from "./notes";

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(
    <StrictMode>
      <App api={notesApi(apiBase())} />
    </StrictMode>,
  );
}
