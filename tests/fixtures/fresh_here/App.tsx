import { useState, useEffect } from "react";

// #309: a dep through a default over a prop the parent allocates is new only
// when the parent renders: a possible loop, never a certain one.
function DepParentAllocates({ p1 = 0 }) {
  const [s1, setS1] = useState(0);
  useEffect(() => { setS1({ v: 0 }); }, [p1]);
  return <div>{String(s1)}</div>;
}

// The default replaces an explicit `undefined`: a new array at every render
// of the child, a certain loop.
function DepDefaultUsed({ p1 = [] }) {
  const [s1, setS1] = useState(0);
  useEffect(() => { setS1({ v: 0 }); }, [p1]);
  return <div>{String(s1)}</div>;
}

// The same two through a write of the prop: the parent's array is stored
// again when the child re-renders alone; the default's is new each time.
function StoresParentAllocated({ p1 = 0 }) {
  const [s1, setS1] = useState<unknown>(null);
  useEffect(() => { setS1(p1); }, [s1]);
  return <div>{String(s1)}</div>;
}

function StoresDefault({ p1 = [] }) {
  const [s1, setS1] = useState<unknown>(null);
  useEffect(() => { setS1(p1); }, [s1]);
  return <div>{String(s1)}</div>;
}

// A dep the render allocates through a builtin is new at every render.
export function DepMapped() {
  const [s1, setS1] = useState(0);
  const xs = [1, 2].map((x) => x);
  useEffect(() => { setS1({ v: 0 }); }, [xs]);
  return <div>{String(s1)}</div>;
}

export function App() {
  return (
    <div>
      <DepParentAllocates p1={[]} />
      <DepDefaultUsed p1={undefined} />
      <StoresParentAllocated p1={[]} />
      <StoresDefault p1={undefined} />
    </div>
  );
}
