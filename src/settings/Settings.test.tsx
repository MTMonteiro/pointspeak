import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Settings } from "./Settings";

describe("Settings", () => {
  it("mostra o título do app", () => {
    render(<Settings />);
    expect(screen.getByRole("heading", { name: "PointSpeak" })).toBeInTheDocument();
  });
});
