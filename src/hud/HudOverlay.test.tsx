import { act, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { emitTauriEvent } from "../test/tauri-mock";
import { HUD_STATE_EVENT, HudOverlay } from "./HudOverlay";

describe("HudOverlay", () => {
  it("começa em idle", () => {
    render(<HudOverlay />);
    expect(screen.getByRole("status")).toHaveAttribute("data-state", "idle");
  });

  it("segue o estado emitido pelo backend", async () => {
    render(<HudOverlay />);
    await act(async () => {
      await Promise.resolve();
      emitTauriEvent(HUD_STATE_EVENT, "recording");
    });
    expect(screen.getByRole("status")).toHaveAttribute("data-state", "recording");
  });
});
