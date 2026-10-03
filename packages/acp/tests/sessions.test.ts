import { expect, test } from "vite-plus/test";
import { mapSessionListResponse } from "../src/AcpConnection.js";

test("maps session list entries and lock metadata", () => {
  expect(
    mapSessionListResponse({
      sessions: [
        {
          sessionId: "s1",
          cwd: "/work",
          title: "One",
          updatedAt: "2024-01-01T00:00:00.000Z",
          _meta: { "cognition.ai/isLocked": true, "cognition.ai/lockHolderPid": 42 },
        },
        { sessionId: "s2", cwd: "/other" },
      ],
    }),
  ).toEqual([
    {
      sessionId: "s1",
      cwd: "/work",
      title: "One",
      updatedAt: "2024-01-01T00:00:00.000Z",
      locked: true,
      lockHolderPid: 42,
    },
    {
      sessionId: "s2",
      cwd: "/other",
      title: "",
      updatedAt: "",
      locked: false,
      lockHolderPid: null,
    },
  ]);
});

test("coerces a non-boolean lock flag to false", () => {
  const [session] = mapSessionListResponse({
    sessions: [{ sessionId: "s1", cwd: "/work", _meta: { "cognition.ai/isLocked": "yes" } }],
  });
  expect(session?.locked).toBe(false);
});

test("returns an empty list when sessions are missing", () => {
  expect(mapSessionListResponse({})).toEqual([]);
});
