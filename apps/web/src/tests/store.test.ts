import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { getRecents } from "../lib/recents";
import {
  sepiaStore,
  setCreateCwd,
  setCwd,
  setDetailsFor,
  setNewProjectFor,
  setReplyTo,
  setSelectedId,
  setSettingsOpen,
} from "../lib/store";

const store = new Map<string, string>();

beforeEach(() => {
  store.clear();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => store.get(key) ?? null,
    setItem: (key: string, value: string) => {
      store.set(key, String(value));
    },
    removeItem: (key: string) => {
      store.delete(key);
    },
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("sepiaStore setters", () => {
  it("setSelectedId records the session in recents", () => {
    setSelectedId("sess-1");
    expect(sepiaStore.state.selectedId).toBe("sess-1");
    expect(getRecents()).toContain("sess-1");
    setSelectedId(null);
    expect(sepiaStore.state.selectedId).toBeNull();
    // Selecting null does not write a "null" recent.
    expect(getRecents()).not.toContain("null");
  });

  it("setSettingsOpen tracks the target section", () => {
    setSettingsOpen(true, "keybinds");
    expect(sepiaStore.state.settingsOpen).toBe(true);
    expect(sepiaStore.state.settingsSection).toBe("keybinds");

    setSettingsOpen(true);
    expect(sepiaStore.state.settingsSection).toBeNull();

    setSettingsOpen(false, "ignored");
    expect(sepiaStore.state.settingsOpen).toBe(false);
    expect(sepiaStore.state.settingsSection).toBeNull();
  });

  it("setDetailsFor opens and closes the details drawer", () => {
    setDetailsFor({ id: "s1", rename: true });
    expect(sepiaStore.state.detailsFor).toEqual({ id: "s1", rename: true });
    setDetailsFor(null);
    expect(sepiaStore.state.detailsFor).toBeNull();
  });

  it("cwd setters are independent slots", () => {
    setCwd("/work/a");
    setCreateCwd("/work/b");
    expect(sepiaStore.state.cwd).toBe("/work/a");
    expect(sepiaStore.state.createCwd).toBe("/work/b");
    setCwd(null);
    setCreateCwd(null);
    expect(sepiaStore.state.cwd).toBeNull();
    expect(sepiaStore.state.createCwd).toBeNull();
  });

  it("setNewProjectFor tracks the session id", () => {
    setNewProjectFor("s9");
    expect(sepiaStore.state.newProjectFor).toBe("s9");
    setNewProjectFor(null);
    expect(sepiaStore.state.newProjectFor).toBeNull();
  });

  it("setReplyTo sets and clears the quote", () => {
    setReplyTo({ role: "assistant", content: "quoted" });
    expect(sepiaStore.state.replyTo?.content).toBe("quoted");
    setReplyTo(null);
    expect(sepiaStore.state.replyTo).toBeNull();
  });
});
