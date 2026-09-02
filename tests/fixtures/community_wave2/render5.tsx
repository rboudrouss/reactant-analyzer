import { useMemo } from "react";

export function Render5Fires({ rowsJson }) {
  const rows = JSON.parse(rowsJson);
  return <ul>{rows.length}</ul>;
}

export function Render5Silent({ rowsJson }) {
  const rows = useMemo(() => JSON.parse(rowsJson), [rowsJson]);
  return <ul>{rows.length}</ul>;
}
