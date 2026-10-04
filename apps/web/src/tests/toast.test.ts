import { describe, expect, it, vi } from "vite-plus/test";
import { toast } from "sonner";
import { toastError, toastSuccess } from "../lib/toast";

vi.mock("sonner", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}));

const success = vi.mocked(toast.success);
const error = vi.mocked(toast.error);

describe("toastSuccess", () => {
  it("forwards the message to sonner", () => {
    toastSuccess("Saved");
    expect(success).toHaveBeenCalledWith("Saved");
  });
});

describe("toastError", () => {
  it("includes the error's message as the description", () => {
    toastError("Delete failed", new Error("409 conflict"));
    expect(error).toHaveBeenCalledWith("Delete failed", { description: "409 conflict" });
  });

  it("omits the description for non-Error and empty-message throws", () => {
    toastError("Oops", "a string rejection");
    expect(error).toHaveBeenCalledWith("Oops", { description: undefined });
    toastError("Oops", new Error(""));
    expect(error).toHaveBeenCalledWith("Oops", { description: undefined });
    toastError("Oops");
    expect(error).toHaveBeenCalledWith("Oops", { description: undefined });
  });
});
