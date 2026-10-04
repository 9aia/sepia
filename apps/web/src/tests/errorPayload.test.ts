import { describe, expect, it } from "vite-plus/test";
import { parseErrorPayload, prettifyCode } from "../lib/errorPayload";

describe("parseErrorPayload", () => {
  it("parses an error object envelope", () => {
    expect(parseErrorPayload('{"error":{"code":"rate_limit","message":"slow down"}}')).toEqual({
      code: "rate_limit",
      message: "slow down",
    });
  });

  it("parses a string error member", () => {
    expect(parseErrorPayload('{"error":"boom happened","code":"internal"}')).toEqual({
      code: "internal",
      message: "boom happened",
    });
  });

  it("parses a flat payload carrying an error marker", () => {
    expect(parseErrorPayload('{"code":"timeout","message":"timed out"}')).toEqual({
      code: "timeout",
      message: "timed out",
    });
    expect(parseErrorPayload('{"type":"error","message":"nope"}')).toEqual({
      message: "nope",
    });
  });

  it("does not swallow a bare {message} payload", () => {
    expect(parseErrorPayload('{"message":"just data"}')).toBeNull();
  });

  it("ignores non-error JSON, arrays, and plain text", () => {
    expect(parseErrorPayload('{"foo":1}')).toBeNull();
    expect(parseErrorPayload('{"error":{"detail":"no message key"}}')).toBeNull();
    expect(parseErrorPayload('[{"error":{"message":"x"}}]')).toBeNull();
    expect(parseErrorPayload("not json")).toBeNull();
    expect(parseErrorPayload('{"error": unclosed')).toBeNull();
  });
});

describe("prettifyCode", () => {
  it("title-cases snake codes", () => {
    expect(prettifyCode("rate_limit_exceeded")).toBe("Rate Limit Exceeded");
    expect(prettifyCode("TIMEOUT")).toBe("Timeout");
  });
});
