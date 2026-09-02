import { useEffect } from "react";

export function Async15Fires({ router, user }) {
  if (!user) {
    router.push("/login");
  }
  return <div>hi</div>;
}

export function Async15Silent({ router, user }) {
  useEffect(() => {
    if (!user) {
      router.push("/login");
    }
  }, [user, router]);
  return <div>hi</div>;
}
