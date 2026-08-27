// No directive of its own, and the only thing importing it is the server
// layout — so Next compiles it into the server graph and `usePathname` here
// is the transitive form of the same bug.
import { usePathname } from "next/navigation";
import { useNavItems } from "@/hooks/use-nav-items";

export function Sidebar() {
  const pathname = usePathname();
  const items = useNavItems();
  return <nav data-active={pathname}>{items.length}</nav>;
}
