import { useState } from "react";
import { Row } from "./row";

// Two files define `Demo`, so the registry disambiguates both to `Demo@<file>`.
// `setFilter` is this component's own setter, wrapped: calling it during render
// is not a *cross*-component write, and the salted spelling must not make the
// self-ownership check fail.
export function Demo() {
  const [filters, setFilters] = useState({ open: true });
  const setFilter = (key: string) => (checked: boolean) =>
    setFilters((c) => ({ ...c, [key]: checked }));
  return <Row checked={filters.open} onChange={setFilter("open")} />;
}
