import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useDeadlineReached } from "../../src/hooks/useDeadlineReached";

describe("useDeadlineReached", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("rerenders at a deadline even when it exceeds the browser timeout limit", () => {
    const nowMs = 1_000_000;
    const deadlineMs = nowMs + 2_147_483_647 + 2_000;
    vi.useFakeTimers();
    vi.setSystemTime(nowMs);

    const { result } = renderHook(() => useDeadlineReached(deadlineMs));
    expect(result.current).toBe(false);

    act(() => vi.advanceTimersByTime(2_147_483_647));
    expect(result.current).toBe(false);

    act(() => vi.advanceTimersByTime(1_999));
    expect(result.current).toBe(false);
    act(() => vi.advanceTimersByTime(1));
    expect(result.current).toBe(true);
  });
});
