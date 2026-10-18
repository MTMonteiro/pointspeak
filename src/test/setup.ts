import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";
import { resetTauriMock } from "./tauri-mock";

afterEach(() => {
  cleanup();
  resetTauriMock();
  window.history.replaceState(null, "", "/");
});
