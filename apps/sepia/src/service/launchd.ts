import type { ServiceBackend } from "./backend.js";

export const launchdBackend: ServiceBackend = {
  id: "launchd",
  unitPath: () => "",
  render: () => "",
  install: async () => {},
  uninstall: async () => {},
  status: async () => ({ installed: false, enabled: false, active: false, detail: "" }),
  restart: async () => {},
  logs: async () => 0,
};
