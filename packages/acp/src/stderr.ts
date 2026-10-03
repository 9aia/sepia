const RING_LIMIT = 100;
const LINE_LIMIT = 500;

export interface StderrSource {
  recent(): ReadonlyArray<string>;
}

/**
 * Bounded tail of an agent's stderr. Lines are prefixed and truncated, kept in
 * a fixed-size ring, and only forwarded to the process when debugging.
 */
export class StderrTail implements StderrSource {
  private readonly lines: string[] = [];
  private readonly prefix: string;
  private readonly debug: boolean;
  private readonly sink: (line: string) => void;
  private remainder = "";

  constructor(options: {
    readonly id: string;
    readonly debug?: boolean;
    readonly sink?: (line: string) => void;
  }) {
    this.prefix = `[agent:${options.id}]`;
    this.debug = options.debug === true;
    this.sink = options.sink ?? ((line) => process.stderr.write(`${line}\n`));
  }

  push(chunk: string): void {
    const parts = (this.remainder + chunk).split("\n");
    this.remainder = parts.pop() ?? "";
    for (const part of parts) this.append(part);
  }

  recent(): ReadonlyArray<string> {
    return [...this.lines];
  }

  private append(line: string): void {
    const body = line.length > LINE_LIMIT ? line.slice(0, LINE_LIMIT) : line;
    const formatted = `${this.prefix} ${body}`;
    this.lines.push(formatted);
    if (this.lines.length > RING_LIMIT) this.lines.shift();
    if (this.debug) this.sink(formatted);
  }
}
