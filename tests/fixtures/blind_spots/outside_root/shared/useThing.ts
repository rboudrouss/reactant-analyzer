import { useEffect, useState } from "react";

// An infinite loop nobody sees when the run is pointed at `app/` alone.
export function useThing() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  });
  return n;
}
