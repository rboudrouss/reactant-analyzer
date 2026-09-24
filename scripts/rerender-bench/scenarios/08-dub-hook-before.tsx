import { useState } from "react";

// dubinc/dub#3633 (reduced). The filter's search state hides inside a custom
// hook called by the table page, so each keystroke re-renders the table.
function useCustomerFilters() {
  const [search, setSearch] = useState("");
  const [selectedFilter, setSelectedFilter] = useState<string | null>(null);
  const filters = ["country", "plan"].filter((f) => f.includes(search));
  return { filters, selectedFilter, setSearch, setSelectedFilter };
}
function FilterSelect({ filters, onSearchChange, onSelect }: { filters: string[]; onSearchChange: (s: string) => void; onSelect: (f: string) => void }) {
  return (
    <div>
      <input id="search" onChange={(e) => onSearchChange(e.target.value)} />
      {filters.map((f) => <button key={f} onClick={() => onSelect(f)}>{f}</button>)}
    </div>
  );
}
function TableRow({ name }: { name: string }) { return <tr><td>{name}</td></tr>; }
function Table({ rows }: { rows: string[] }) { return <table><tbody>{rows.map((r) => <TableRow key={r} name={r} />)}</tbody></table>; }
const ROWS = ["ada", "bob", "cy"];

export default function CustomersTable() {
  const { filters, setSearch, setSelectedFilter } = useCustomerFilters();
  return (
    <div>
      <FilterSelect filters={filters} onSearchChange={setSearch} onSelect={setSelectedFilter} />
      <Table rows={ROWS} />
    </div>
  );
}
export const interaction = "type 3 chars in filter search";
export async function interact(ui: any) { await ui.type("#search", "cou"); }
