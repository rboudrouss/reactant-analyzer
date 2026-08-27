"use client";

import { useEffect, useState } from "react";

export function Counter({ start }: { start: number }) {
  const [n, setN] = useState(start);
  useEffect(() => {
    setN(n + 1);
  });
  return <button onClick={() => setN(n + 1)}>{n}</button>;
}
