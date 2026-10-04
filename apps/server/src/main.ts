import { parseEnv, startServer } from "./serve";

try {
  await startServer(parseEnv());
} catch (error) {
  console.error(`sepia-server: ${error instanceof Error ? error.message : String(error)}`);
  process.exit(1);
}
