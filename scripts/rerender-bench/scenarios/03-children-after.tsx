import { useState, type ReactNode } from "react";

// Fix ("lift content up"): the caller builds <Heavy/> and passes it as
// children. The element object is created in App's render, which does not
// re-run, so React bails out on it.
function Heavy() { return <section>{[0, 1, 2].map((i) => <span key={i}>{i}</span>)}</section>; }
function HoverCard({ children }: { children: ReactNode }) {
  const [hover, setHover] = useState(false);
  return (
    <div id="card" className={hover ? "on" : "off"}
         onMouseOver={() => setHover(true)} onMouseOut={() => setHover(false)}>
      {children}
    </div>
  );
}
export default function App() { return <HoverCard><Heavy /></HoverCard>; }
export const interaction = "hover in/out x3";
export async function interact(ui: any) {
  for (let i = 0; i < 3; i++) { await ui.fire("#card", "mouseover"); await ui.fire("#card", "mouseout"); }
}
