import { useState } from "react";

export function Demo() {
  const [x, setX] = useState(0);
  return <div>{x}</div>;
}
