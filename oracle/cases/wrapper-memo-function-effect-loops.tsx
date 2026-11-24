// oracle: react=loop
// #64. `memo(function Counter() {…})` renders the function: its effect, with
// no dependency array, writes a new value after every commit. The wrapped
// body was no component, and the loop had no claim. (The `displayName` names
// it for the runner's counter: the compiler renames the inner `Counter`.)
import { memo, useEffect, useState } from "react";

const Counter = memo(function Counter() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  });
  return <i>{n}</i>;
});
Counter.displayName = "Counter";

export function App() {
  return <Counter />;
}
