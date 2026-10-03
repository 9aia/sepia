const encoder = new TextEncoder();

export const DEFAULT_KEEPALIVE_MS = 15_000;
export const DEFAULT_MAX_PENDING = 2_000;
export const DEFAULT_MAX_OUTSTANDING_BYTES = 8 * 1024 * 1024;

/** `SEPIA_SSE_KEEPALIVE_MS=0` disables the keep-alive frames. */
export const keepAliveMsFromEnv = (raw: string | undefined): number => {
  if (raw === undefined || raw === "") return DEFAULT_KEEPALIVE_MS;
  const value = Number(raw);
  return Number.isFinite(value) && value >= 0 ? Math.floor(value) : DEFAULT_KEEPALIVE_MS;
};

export interface SseChannelOptions {
  readonly keepAliveMs?: number;
  readonly maxPending?: number;
  readonly maxOutstandingBytes?: number;
  /** Invoked once when the channel closes, errors, or trips a safety limit. */
  readonly onTerminate?: (reason: string | null) => void;
}

/**
 * Owns an SSE `ReadableStream`: buffers frames until the stream starts, emits
 * periodic keep-alive comments, and closes the stream when a client stops
 * draining or the pre-start buffer overflows.
 */
export class SseChannel {
  private readonly pending: string[] = [];
  private readonly keepAliveMs: number;
  private readonly maxPending: number;
  private readonly maxOutstandingBytes: number;
  private readonly onTerminate: (reason: string | null) => void;
  private controller: ReadableStreamDefaultController<Uint8Array> | null = null;
  private timer: ReturnType<typeof setInterval> | null = null;
  private terminated = false;
  private terminateReason: string | null = null;
  private outstanding = 0;

  constructor(options: SseChannelOptions = {}) {
    this.keepAliveMs = options.keepAliveMs ?? DEFAULT_KEEPALIVE_MS;
    this.maxPending = options.maxPending ?? DEFAULT_MAX_PENDING;
    this.maxOutstandingBytes = options.maxOutstandingBytes ?? DEFAULT_MAX_OUTSTANDING_BYTES;
    this.onTerminate = options.onTerminate ?? (() => {});
  }

  get isTerminated(): boolean {
    return this.terminated;
  }

  start(controller: ReadableStreamDefaultController<Uint8Array>): void {
    if (this.terminated) {
      // Events that arrived between `push` and `start` must not be lost when a
      // fast turn already finished — flush them before closing.
      for (const frame of this.pending.splice(0)) {
        try {
          controller.enqueue(encoder.encode(frame));
        } catch {
          break;
        }
      }
      try {
        if (this.terminateReason === null) controller.close();
        else controller.error(new Error(this.terminateReason));
      } catch {
        // Already closed by the consumer.
      }
      return;
    }
    this.controller = controller;
    for (const frame of this.pending.splice(0)) this.enqueue(frame);
    if (this.keepAliveMs > 0) {
      this.timer = setInterval(() => this.ping(), this.keepAliveMs);
      this.timer.unref?.();
    }
  }

  push(frame: string): void {
    if (this.terminated) return;
    if (this.controller === null) {
      if (this.pending.length >= this.maxPending) {
        this.terminate("pending frame buffer overflow");
        return;
      }
      this.pending.push(frame);
      return;
    }
    this.enqueue(frame);
  }

  /** The consumer drained the queue; reset the outstanding byte estimate. */
  onPull(): void {
    this.outstanding = 0;
  }

  close(): void {
    this.terminate(null);
  }

  private enqueue(frame: string): void {
    if (this.controller === null) return;
    const bytes = encoder.encode(frame);
    if (this.outstanding + bytes.byteLength > this.maxOutstandingBytes) {
      this.terminate("client is not draining the stream");
      return;
    }
    this.outstanding += bytes.byteLength;
    try {
      this.controller.enqueue(bytes);
    } catch {
      this.terminate("stream already closed");
    }
  }

  private ping(): void {
    if (this.controller === null || this.terminated) return;
    try {
      this.controller.enqueue(encoder.encode(": ping\n\n"));
    } catch {
      this.terminate("stream already closed");
    }
  }

  private terminate(reason: string | null): void {
    if (this.terminated) return;
    this.terminated = true;
    this.terminateReason = reason;
    if (this.timer !== null) {
      clearInterval(this.timer);
      this.timer = null;
    }
    try {
      if (reason === null) this.controller?.close();
      else this.controller?.error(new Error(reason));
    } catch {
      // Already closed by the consumer.
    }
    this.onTerminate(reason);
  }
}
