import { useState } from "react";
import { useSync } from "./useSync";

export function A() {
  const [x, setX] = useState(0);
  const [y, setY] = useState(0);
  useSync(x, setY);
  useSync(y, setX);
  return (
    <div>
      {x}
      {y}
    </div>
  );
}
