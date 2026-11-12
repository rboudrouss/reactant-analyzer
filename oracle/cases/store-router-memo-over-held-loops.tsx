// oracle: react=loop
// A memo over the location feeds the effect's deps: every navigation makes a
// new `location`, so the memo recomputes a new object, and the effect that
// navigates again runs after every commit, for ever.
import { useEffect, useMemo, useState } from "react";
import { MemoryRouter, useLocation, useNavigate } from "react-router";

function Panel() {
  const location = useLocation();
  const navigate = useNavigate();
  const path = useMemo(() => ({ p: location.pathname }), [location]);
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c) => c + 1);
    navigate("/x/" + Math.random());
  }, [path]);
  return <p>{n}</p>;
}

export function App() {
  return (
    <MemoryRouter>
      <Panel />
    </MemoryRouter>
  );
}
