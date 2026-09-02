import { useEffect } from "react";

export function Eff10Fires({ socket, room }) {
  useEffect(() => {
    socket.join(room);
  }, [socket, room]);
  return <div />;
}

export function Eff10Silent({ socket, room }) {
  useEffect(() => {
    socket.join(room);
    return () => socket.leave(room);
  }, [socket, room]);
  return <div />;
}
