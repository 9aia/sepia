import { expect, test } from "vite-plus/test";
import { createPermissionBroker } from "../src/permissions.js";

const params = (toolCallId: string) => ({
  sessionId: "s1",
  toolCall: { toolCallId, title: "Run command" },
  options: [{ optionId: "allow", name: "Allow", kind: "allow_once" }],
});

test("respondToPermission resolves with the selected option", async () => {
  const broker = createPermissionBroker();
  const pending = broker.begin(params("t1"));
  expect(broker.respond(pending.request.requestId, "allow")).toBe(true);
  await expect(pending.response).resolves.toEqual({
    outcome: { outcome: "selected", optionId: "allow" },
  });
});

test("a null optionId cancels the request", async () => {
  const broker = createPermissionBroker();
  const pending = broker.begin(params("t1"));
  expect(broker.respond(pending.request.requestId, null)).toBe(true);
  await expect(pending.response).resolves.toEqual({ outcome: { outcome: "cancelled" } });
});

test("an unknown id settles nothing and leaves pending requests untouched", async () => {
  const broker = createPermissionBroker();
  const first = broker.begin(params("t1"));
  const second = broker.begin(params("t2"));

  expect(broker.respond("unknown", "allow")).toBe(false);

  let firstSettled = false;
  void first.response.then(() => {
    firstSettled = true;
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(firstSettled).toBe(false);

  expect(broker.respond(first.request.requestId, "allow")).toBe(true);
  await expect(first.response).resolves.toEqual({
    outcome: { outcome: "selected", optionId: "allow" },
  });

  expect(broker.respond(second.request.requestId, null)).toBe(true);
  await expect(second.response).resolves.toEqual({ outcome: { outcome: "cancelled" } });
});

test("responding with an unknown id and nothing pending is a no-op", () => {
  const broker = createPermissionBroker();
  expect(broker.respond("unknown", "allow")).toBe(false);
});

test("failAll rejects every pending request", async () => {
  const broker = createPermissionBroker();
  const first = broker.begin(params("t1"));
  const second = broker.begin(params("t2"));

  broker.failAll(new Error("agent exited"));

  await expect(first.response).rejects.toThrow("agent exited");
  await expect(second.response).rejects.toThrow("agent exited");
  expect(broker.respond(first.request.requestId, "allow")).toBe(false);
});
