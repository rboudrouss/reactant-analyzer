import { createContext, useContext, useState } from "react";

// A provider is followed, not a barrier: what it wraps re-renders with its
// owner, and only the consumers of the context need to (#145).
const Ctx = createContext("");
function Chart() { return <svg><rect /></svg>; }
function Leaf() { const v = useContext(Ctx); return <p>{v}</p>; }
function useText() { return useContext(Ctx); }
function HookLeaf() { const t = useText(); return <p>{t}</p>; }

// `Mid` and `Chart` re-render for nothing, `Leaf` reads the value.
function Mid() { return <div><Leaf /><Chart /></div>; }
export function Cascade() {
  const [v, setV] = useState("");
  return (
    <Ctx.Provider value={v}>
      <input value={v} onChange={(e) => setV(e.target.value)} />
      <Mid />
    </Ctx.Provider>
  );
}

// Read through a custom hook, bound or inline, the value still makes
// `HookLeaf` and `InlineLeaf` consumers.
function InlineLeaf() { return <p>{useText()}</p>; }
function Static() { return <div><Chart /></div>; }
export function Hooked() {
  const [v, setV] = useState("");
  return (
    <Ctx.Provider value={v}>
      <input value={v} onChange={(e) => setV(e.target.value)} />
      <HookLeaf />
      <InlineLeaf />
      <Static />
    </Ctx.Provider>
  );
}
