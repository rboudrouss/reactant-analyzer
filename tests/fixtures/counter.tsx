import React, { useState, useEffect, useMemo, useCallback } from "react";

// Clean counter — no issues expected.

function Counter() {
  const [count, setCount] = useState(0);
  const [step, setStep] = useState(1);

  // Effect correctly lists count in deps.
  useEffect(() => {
    document.title = String(count);
  }, [count]);

  // useMemo correctly depends on count and step.
  const next = useMemo(() => count + step, [count, step]);

  // useCallback correctly lists setCount in deps (stable, so empty is fine too).
  const increment = useCallback(() => {
    setCount((n) => n + step);
  }, [step]);

  const reset = useCallback(() => {
    setCount(0);
    setStep(1);
  }, []);

  return (
    <div>
      <p>{count}</p>
      <p>Next: {next}</p>
      <button onClick={increment}>+{step}</button>
      <button onClick={reset}>Reset</button>
      <input
        type="number"
        value={step}
        onChange={(e) => setStep(Number(e.target.value))}
      />
    </div>
  );
}

// Custom hook wrapping a basic timer.
function useInterval(callback, delay) {
  useEffect(() => {
    const id = setInterval(callback, delay);
    return () => clearInterval(id);
  }, [callback, delay]);
}

function Stopwatch() {
  const [elapsed, setElapsed] = useState(0);
  const [running, setRunning] = useState(false);

  const tick = useCallback(() => {
    setElapsed((n) => n + 1);
  }, []);

  useInterval(tick, running ? 100 : null);

  return (
    <div>
      <p>{elapsed}</p>
      <button onClick={() => setRunning((r) => !r)}>
        {running ? "Pause" : "Start"}
      </button>
      <button onClick={() => setElapsed(0)}>Reset</button>
    </div>
  );
}
