// oracle: react=loop
// oracle: known=fn #298
// react-router's real `useLocation` and `useNavigate`: `navigate` pushes a new
// location, so `location` is a new object after every commit and the effect on
// `[location]` runs again, for ever. The analyzer does not see the navigation
// as a write to the location the effect depends on (#298).
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
