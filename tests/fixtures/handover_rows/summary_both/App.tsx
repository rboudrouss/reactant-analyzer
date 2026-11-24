// A Warning shown and an Info hidden: the summary counts the Warning, the
// line under it what `--info` would add.
import { useState } from "react";
import { register } from "some-pkg";

declare function local(x: unknown): void;

export function Both() {
  const [n, setN] = useState(0);
  const [m, setM] = useState(0);
  register(setN);
  local(setM);
  return <div>{n + m}</div>;
}
