// Local utility called at statement-level — gets inlined into the effect
// body so the engine sees the call site (smoke test for Phase 3 splicing).

function bump(setter, value) {
  setter(value);
}

function Counter() {
  const [c, setC] = useState(0);
  useEffect(() => {
    bump(setC, 1);
  }, []);
  return <div>{c}</div>;
}
