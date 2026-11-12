// oracle: react=loop
// The effect on `[location]` navigates through a callback it hands to an
// unknown function, which runs it at once: the location is a new object after
// every commit, and the effect runs again, for ever. The navigation sits in a
// closure of unknown phase, which still writes the location.
import { useEffect, useState } from "react";
import { MemoryRouter, useLocation, useNavigate } from "react-router";
import { runNow } from "ext";

function Tracker() {
  const location = useLocation();
  const navigate = useNavigate();
  const [seen, setSeen] = useState(0);
  useEffect(() => {
    setSeen((c) => c + 1);
    runNow(() => navigate("/x/" + Math.random()));
  }, [location]);
  return <p>{seen}</p>;
}

export function App() {
  return (
    <MemoryRouter>
      <Tracker />
    </MemoryRouter>
  );
}
