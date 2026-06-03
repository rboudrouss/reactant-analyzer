import React, { useState, useEffect } from "react";

// ✅ onClick handler normal — pas d'InfiniteLoop (le handler ne tourne que sur
// input externe, il n'est pas dans le cycle render→effect→setState→render).
function Counter() {
  const [count, setCount] = useState(0);
  return <button onClick={() => setCount(count + 1)}>{count}</button>;
}

// ✅ onChange sur input contrôlé — sain.
function ControlledInput() {
  const [value, setValue] = useState("");
  return <input value={value} onChange={(e) => setValue(e.target.value)} />;
}

// ✅ Handler avec prop non-FnLit (référence externe) — pas extrait comme Handler,
// pas analysé, pas de faux positif.
function WithExternalHandler({ onClick }: { onClick: () => void }) {
  return <button onClick={onClick}>click</button>;
}

// ✅ Plusieurs handlers sur le même élément.
function MultiHandler() {
  const [n, setN] = useState(0);
  return (
    <div onMouseEnter={() => setN(1)} onMouseLeave={() => setN(0)}>
      {n}
    </div>
  );
}

// ✅ Handler sur élément JSX imbriqué.
function NestedHandler() {
  const [active, setActive] = useState(false);
  return (
    <div>
      <button onClick={() => setActive(!active)}>toggle</button>
    </div>
  );
}

// ✅ setCount(c + 1) dans un handler — sain (l'utilisateur clique, pas une boucle).
function IncrementOnClick() {
  const [count, setCount] = useState(0);
  return (
    <button onClick={() => setCount(count + 1)}>
      {count}
    </button>
  );
}

function InfiniteLoop() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    if (count > 1) {
      setCount(count + 1); // ❌ boucle infinie (setState dans useEffect sans deps ou avec deps qui inclut la valeur modifiée)
    }
  }, [count]);
  return (
    <button onClick={() => setCount(count + 1)}>
      {count}
    </button>
  );
}
