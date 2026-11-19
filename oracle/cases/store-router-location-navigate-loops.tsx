// oracle: react=loop
// react-router's real `useLocation` and `useNavigate`: `navigate` pushes a new
// location, so `location` is a new object after every commit and the effect on
// `[location]` runs again, for ever. The navigation writes the store the
// effect's dep reads (#298).
import { useEffect, useState } from "react";
import { MemoryRouter, useLocation, useNavigate } from "react-router";

function Tracker() {
  const location = useLocation();
  const navigate = useNavigate();
  const [seen, setSeen] = useState(0);
  useEffect(() => {
    setSeen((c) => c + 1);
    navigate("/x/" + Math.random());
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
