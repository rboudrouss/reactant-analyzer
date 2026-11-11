// oracle: react=converges
// The effect only copies the current location into state: nothing navigates, so
// `location` keeps its identity across the re-render the copy causes and the
// effect does not run again.
import { useEffect, useState } from "react";
import { MemoryRouter, useLocation } from "react-router";

function Tracker() {
  const location = useLocation();
  const [seen, setSeen] = useState("");
  useEffect(() => {
    setSeen(location.pathname);
  }, [location]);
  return <p>{seen}</p>;
}

export function App() {
  return (
    <MemoryRouter initialEntries={["/a"]}>
      <Tracker />
    </MemoryRouter>
  );
}
