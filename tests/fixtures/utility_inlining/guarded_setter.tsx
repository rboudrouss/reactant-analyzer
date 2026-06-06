// `doOrNot` wraps the setter call behind a guard. After Phase 3 inlining,
// the guard is visible in the effect body. The setter is invoked via an
// inner FnLit, which is still opaque to the engine (Phase 3 inlines
// statement-level calls only — the FnLit invocation stays a Call → Top).
// Smoke test: the analyzer must not crash and must produce a Counter result.

function doOrNot(fn) {
  if (!LAUNCH) return;
  fn();
}

function Counter() {
  const [c, setC] = useState(0);
  useEffect(() => {
    doOrNot(() => setC(c + 1));
  }, []);
  return <div>{c}</div>;
}
