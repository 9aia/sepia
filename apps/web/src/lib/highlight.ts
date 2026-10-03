import { defaultHighlighter } from "@tanstack/highlight";
import { createTanStackMarkdownHighlighter } from "@tanstack/highlight/markdown";
import { createThemeCss } from "@tanstack/highlight/theme";
import { githubDarkTheme } from "@tanstack/highlight/themes/github-dark";

/** TanStack-Markdown-compatible code highlighter for fenced blocks. */
export const codeHighlighter = createTanStackMarkdownHighlighter(defaultHighlighter);

/** Theme CSS scoped to the history pane; render once inside a `<style>` tag. */
export const highlightThemeCss = createThemeCss({
  dark: githubDarkTheme,
  darkSelector: ".history__content",
});
