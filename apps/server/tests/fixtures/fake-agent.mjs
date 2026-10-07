// Minimal ACP agent over newline-delimited JSON-RPC on stdio, used by tests/e2e.ts.
import readline from "node:readline";

const send = (message) => process.stdout.write(`${JSON.stringify(message)}\n`);

const rl = readline.createInterface({ input: process.stdin });
rl.on("line", (line) => {
  if (line.trim() === "") return;
  let message;
  try {
    message = JSON.parse(line);
  } catch {
    return;
  }
  const { id, method, params } = message;
  switch (method) {
    case "initialize":
      send({
        jsonrpc: "2.0",
        id,
        result: {
          protocolVersion: params?.protocolVersion ?? 1,
          agentCapabilities: {
            loadSession: true,
            sessionCapabilities: { list: {}, delete: {} },
          },
        },
      });
      break;
    case "session/new":
      send({ jsonrpc: "2.0", id, result: { sessionId: "e2e-session" } });
      break;
    case "session/load":
      send({ jsonrpc: "2.0", id, result: {} });
      break;
    case "session/list":
      send({ jsonrpc: "2.0", id, result: { sessions: [] } });
      break;
    case "session/delete":
      send({ jsonrpc: "2.0", id, result: {} });
      break;
    case "session/prompt":
      send({
        jsonrpc: "2.0",
        method: "session/update",
        params: {
          sessionId: params?.sessionId,
          update: {
            sessionUpdate: "agent_message_chunk",
            content: { type: "text", text: "ok" },
          },
        },
      });
      send({ jsonrpc: "2.0", id, result: { stopReason: "end_turn" } });
      break;
    default:
      if (id !== undefined && id !== null) {
        send({
          jsonrpc: "2.0",
          id,
          error: { code: -32601, message: `unknown method ${String(method)}` },
        });
      }
  }
});
