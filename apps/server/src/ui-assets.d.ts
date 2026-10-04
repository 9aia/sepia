// Types for the generated embedded-UI manifest (ui.assets.gen.ts, produced by
// tools/build-binary.ts). tools/build-binary.ts stages apps/web/dist/client at
// apps/server/ui-dist/; every file there is imported with `type: "file"` and
// the binding is its runtime path — a real path in source mode, a `/$bunfs/`
// path inside a `bun --compile` binary.
declare module "../ui-dist/*" {
  const path: string;
  export default path;
}
