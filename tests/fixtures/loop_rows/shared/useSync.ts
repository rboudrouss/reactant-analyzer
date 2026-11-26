import { useEffect } from "react";

// One effect site two components inline, each closing a cycle of its own.
export function useSync(a: number, setB: (n: number) => void) {
  useEffect(() => {
    setB(a + 1);
  }, [a]);
}
