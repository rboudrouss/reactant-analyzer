// A clean Server Component: no hooks, so nothing to report.
import { loadStats } from "@/lib/stats";

export default async function DashboardPage() {
  const stats = await loadStats();
  return <section>{stats.total}</section>;
}
