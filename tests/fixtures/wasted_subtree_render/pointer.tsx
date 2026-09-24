import { useEffect, useState } from "react";

// A mousemove listener registered in an effect is continuous.
function Chart() { return <svg>{[1, 2, 3].map((i) => <rect key={i} />)}</svg>; }
function Legend() { return <ul><li>a</li></ul>; }
export default function Board() {
  const [pos, setPos] = useState({ x: 0, y: 0 });
  useEffect(() => {
    const h = (e: MouseEvent) => setPos({ x: e.clientX, y: e.clientY });
    window.addEventListener("mousemove", h);
    return () => window.removeEventListener("mousemove", h);
  }, []);
  return <div><span>{pos.x}</span><Chart /><Legend /></div>;
}
