/**
 * Lazy facades for streamdown's heaviest plugins. `@streamdown/code` imports
 * shiki (engine + every bundled language's metadata) and `@streamdown/mermaid`
 * imports all of mermaid core at module scope, which lands both in the initial
 * bundle even though they're only needed once a code fence / mermaid diagram
 * actually renders.
 *
 * Both plugin interfaces tolerate deferral: `highlight()` may return null and
 * deliver the result later via its callback (streamdown renders the raw code
 * until then), and `getMermaid().render()` is already awaited. The facades
 * below keep those contracts while putting the real implementations behind
 * dynamic imports.
 */
import type { BundledLanguage, CodeHighlighterPlugin, DiagramPlugin, ThemeInput } from "streamdown";

type CodeModule = typeof import("@streamdown/code");
type MermaidModule = typeof import("@streamdown/mermaid");

// streamdown doesn't re-export these leaf types — derive them structurally
// from the plugin interfaces so the facades stay type-aligned.
type HighlightOptions = Parameters<CodeHighlighterPlugin["highlight"]>[0];
type HighlightResult = NonNullable<ReturnType<CodeHighlighterPlugin["highlight"]>>;
type MermaidConfig = NonNullable<Parameters<DiagramPlugin["getMermaid"]>[0]>;
type MermaidInstance = ReturnType<DiagramPlugin["getMermaid"]>;

let codeModule: Promise<CodeModule> | null = null;
const loadCode = (): Promise<CodeModule> => (codeModule ??= import("@streamdown/code"));

let mermaidModule: Promise<MermaidModule> | null = null;
const loadMermaid = (): Promise<MermaidModule> => (mermaidModule ??= import("@streamdown/mermaid"));

/** Matches `@streamdown/code`'s default theme pair — streamdown reads
 * `plugin.getThemes()` for its `shikiTheme` context default. */
const DEFAULT_THEMES: [ThemeInput, ThemeInput] = ["github-light", "github-dark"];

let supportedLanguages: BundledLanguage[] = [];

export const lazyCode: CodeHighlighterPlugin = {
  name: "shiki",
  type: "code-highlighter",
  getSupportedLanguages: () => supportedLanguages,
  getThemes: () => DEFAULT_THEMES,
  // Optimistic until shiki loads: the real `highlight` resolves unknown
  // languages to "text", so claiming support just means a plain render.
  supportsLanguage: () => true,
  highlight(options: HighlightOptions, callback?: (result: HighlightResult) => void) {
    void loadCode()
      .then((mod) => {
        // @streamdown/code narrows params to shiki's unions; the streamdown
        // interface is the bivariant superset, so view it through that type.
        const plugin: CodeHighlighterPlugin = mod.code;
        supportedLanguages = plugin.getSupportedLanguages();
        // The real plugin answers synchronously from its cache or via the
        // callback — forward both paths through the caller's callback.
        const result = plugin.highlight(options, (r) => callback?.(r));
        if (result !== null) callback?.(result);
      })
      .catch((error: unknown) => {
        console.error("[sepia] Failed to load syntax highlighting:", error);
      });
    // null = "not ready"; streamdown shows unhighlighted code until the
    // callback delivers tokens.
    return null;
  },
};

export const lazyMermaid: DiagramPlugin = {
  name: "mermaid",
  type: "diagram",
  language: "mermaid",
  getMermaid(config?: MermaidConfig): MermaidInstance {
    // Config passed to `getMermaid`/`initialize` accumulates; it's applied to
    // the real (singleton) mermaid instance when the module finishes loading.
    let pending: MermaidConfig | undefined = config;
    return {
      initialize(c: MermaidConfig) {
        pending = { ...pending, ...c };
        void loadMermaid().then((mod) => {
          const plugin: DiagramPlugin = mod.mermaid;
          plugin.getMermaid(pending);
          pending = undefined;
        });
      },
      async render(id: string, source: string) {
        const mod = await loadMermaid();
        const plugin: DiagramPlugin = mod.mermaid;
        const instance = plugin.getMermaid(pending);
        pending = undefined;
        return instance.render(id, source);
      },
    };
  },
};
