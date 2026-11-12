// oracle: react=converges
// The effect on `[location]` writes state, but only a click navigates: with no
// user event the location never moves, and the effect runs once.
import { useEffect, useState } from "react";
import { MemoryRouter, useLocation, useNavigate } from "react-router";

function Panel() {
  const location = useLocation();
  const navigate = useNavigate();
  const [seen, setSeen] = useState(0);
  useEffect(() => {
    setSeen((c) => c + 1);
  }, [location]);
  return <button onClick={() => navigate("/x/" + Math.random())}>{seen}</button>;
}

export function App() {
  return (
    <MemoryRouter>
      <Panel />
    </MemoryRouter>
  );
}
