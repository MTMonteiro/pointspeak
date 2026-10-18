import { vi } from "vitest";

type Handler = (event: { event: string; id: number; payload: unknown }) => void;
type InvokeArgs = Record<string, unknown>;
type InvokeCall = { command: string; args?: InvokeArgs };
type InvokeHandler = (command: string, args?: InvokeArgs) => unknown | Promise<unknown>;

const handlers = new Map<string, Set<Handler>>();
const invokeCalls: InvokeCall[] = [];
let invokeHandler: InvokeHandler = () => undefined;

/** Dispara um evento do Tauri para os `listen` registrados nos testes. */
export function emitTauriEvent(event: string, payload: unknown) {
  for (const handler of handlers.get(event) ?? []) {
    handler({ event, id: 0, payload });
  }
}

export function resetTauriMock() {
  handlers.clear();
  invokeCalls.length = 0;
  invokeHandler = () => undefined;
}

/** Configure command responses and failures for a settings-window test. */
export function setInvokeHandler(handler: InvokeHandler) {
  invokeHandler = handler;
}

export function getInvokeCalls() {
  return [...invokeCalls];
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
  invoke: vi.fn((command: string, args?: InvokeArgs) => {
    invokeCalls.push({ command, args });
    try {
      return Promise.resolve(invokeHandler(command, args));
    } catch (error) {
      return Promise.reject(error);
    }
  }),
}));

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn(() => Promise.resolve("0.1.0")),
}));
