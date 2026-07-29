import { useEffect, useState } from "react";

// Browsers clamp larger timeouts to a signed 32-bit integer. Rechecking long
// deadlines in bounded intervals also makes clock changes and sleep/wake safe.
const MAX_TIMEOUT_MS = 2_147_483_647;

export function useDeadlineReached(deadlineMs: number | null): boolean {
  const [nowMs, setNowMs] = useState(() => Date.now());

  useEffect(() => {
    let timeout: ReturnType<typeof setTimeout> | undefined;

    const refresh = () => {
      const currentNowMs = Date.now();
      setNowMs(currentNowMs);
      if (deadlineMs !== null && currentNowMs < deadlineMs) {
        timeout = setTimeout(refresh, Math.min(deadlineMs - currentNowMs, MAX_TIMEOUT_MS));
      }
    };

    refresh();
    return () => {
      if (timeout !== undefined) clearTimeout(timeout);
    };
  }, [deadlineMs]);

  return deadlineMs !== null && nowMs >= deadlineMs;
}
