import { useState } from "react";

// A wrapper owns hover state and *builds* its heavy content itself:
// each hover toggle re-renders Heavy for nothing.
function Heavy() { return <section>{[0, 1, 2].map((i) => <span key={i}>{i}</span>)}</section>; }
function HoverCard() {
  const [hover, setHover] = useState(false);
  return (
    <div id="card" className={hover ? "on" : "off"}
         onMouseOver={() => setHover(true)} onMouseOut={() => setHover(false)}>
      <Heavy />
    </div>
  );
}
export default function App() { return <HoverCard />; }
export const interaction = "hover in/out x3";
export async function interact(ui: any) {
  for (let i = 0; i < 3; i++) { await ui.fire("#card", "mouseover"); await ui.fire("#card", "mouseout"); }
}
