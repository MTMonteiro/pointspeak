import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "../styles.css";
import { HudOverlay } from "./HudOverlay";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <HudOverlay />
  </StrictMode>,
);
