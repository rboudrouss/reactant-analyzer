import { useState } from "react";

// Expensify/App#25758 (reduced). The composer's text lives in the big compose
// component; every keystroke re-renders all its siblings.
function AttachmentPicker() { return <button>attach</button>; }
function EmojiPickerButton() { return <button>emoji</button>; }
function SendButton({ disabled }: { disabled: boolean }) { return <button disabled={disabled}>send</button>; }
function Suggestions({ comment }: { comment: string }) { return comment.startsWith(":") ? <ul><li>emoji</li></ul> : null; }

export default function ReportActionCompose() {
  const [value, setValue] = useState("");
  const [isCommentEmpty, setIsCommentEmpty] = useState(true);
  const updateComment = (text: string) => {
    setIsCommentEmpty(text.trim() === "");
    setValue(text);
  };
  return (
    <div>
      <AttachmentPicker />
      <textarea id="composer" value={value} onChange={(e) => updateComment(e.target.value)} />
      <Suggestions comment={value} />
      <EmojiPickerButton />
      <SendButton disabled={isCommentEmpty} />
    </div>
  );
}
export const interaction = "type 5 chars";
export async function interact(ui: any) { await ui.type("#composer", "hello"); }
