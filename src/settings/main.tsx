import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "../styles.css";
import { Settings } from "./Settings";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Settings />
  </StrictMode>,
);
