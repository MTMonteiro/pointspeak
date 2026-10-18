import { vi } from "vitest";

type Handler = (event: { event: string; id: number; payload: unknown }) => void;

const handlers = new Map<string, Set<Handler>>();

/** Dispara um evento do Tauri para os `listen` registrados nos testes. */
export function emitTauriEvent(event: string, payload: unknown) {
  for (const handler of handlers.get(event) ?? []) {
    handler({ event, id: 0, payload });
  }
}

export function resetTauriMock() {
  handlers.clear();
}

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((event: string, handler: Handler) => {
    const set = handlers.get(event) ?? new Set<Handler>();
    set.add(handler);
    handlers.set(event, set);
    return Promise.resolve(() => {
      set.delete(handler);
    });
  }),
  emit: vi.fn(() => Promise.resolve()),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve(undefined)),
}));
