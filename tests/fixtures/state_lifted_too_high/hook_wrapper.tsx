import { useState } from "react";

// Fix: a child component calls the hook; the table page no longer does.
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
function CustomersFilters() {
  const { filters, setSearch, setSelectedFilter } = useCustomerFilters();
  return <FilterSelect filters={filters} onSearchChange={setSearch} onSelect={setSelectedFilter} />;
}
function TableRow({ name }: { name: string }) { return <tr><td>{name}</td></tr>; }
function Table({ rows }: { rows: string[] }) { return <table><tbody>{rows.map((r) => <TableRow key={r} name={r} />)}</tbody></table>; }
const ROWS = ["ada", "bob", "cy"];

export default function CustomersTable() {
  return (
    <div>
      <CustomersFilters />
      <Table rows={ROWS} />
    </div>
  );
}
export const interaction = "type 3 chars in filter search";
export async function interact(ui: any) { await ui.type("#search", "cou"); }
