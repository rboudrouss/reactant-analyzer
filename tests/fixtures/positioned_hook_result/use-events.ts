import { useMemo } from "react";

export function useEvents(handler: () => void) {
  const events = { onClick: handler };
  return useMemo(() => ({ events }), [handler]);
}
