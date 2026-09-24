import { useState, type ReactNode } from "react";

// dub `RichTextLinkModalInner`, reduced: the input is built here and handed
// to Modal as children. Only this component can rebuild it, so it uses `text`.
function Modal({ children }: { children: ReactNode }) { return <div role="dialog">{children}</div>; }
function Footer() { return <footer>f</footer>; }
export default function LinkModal() {
  const [text, setText] = useState("");
  return (
    <div>
      <Modal><input value={text} onChange={(e) => setText(e.target.value)} /></Modal>
      <Footer />
    </div>
  );
}
