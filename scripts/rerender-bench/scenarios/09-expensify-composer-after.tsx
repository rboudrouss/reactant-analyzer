import { useState } from "react";

// Fix: the per-keystroke text moves into ComposerWithSuggestions; the parent
// keeps only the boolean, whose setter bails out when the value is unchanged.
function AttachmentPicker() { return <button>attach</button>; }
function EmojiPickerButton() { return <button>emoji</button>; }
function SendButton({ disabled }: { disabled: boolean }) { return <button disabled={disabled}>send</button>; }
function Suggestions({ comment }: { comment: string }) { return comment.startsWith(":") ? <ul><li>emoji</li></ul> : null; }
function ComposerWithSuggestions({ setIsCommentEmpty }: { setIsCommentEmpty: (b: boolean) => void }) {
  const [value, setValue] = useState("");
  const updateComment = (text: string) => {
    setIsCommentEmpty(text.trim() === "");
    setValue(text);
  };
  return (
    <>
      <textarea id="composer" value={value} onChange={(e) => updateComment(e.target.value)} />
      <Suggestions comment={value} />
    </>
  );
}

export default function ReportActionCompose() {
  const [isCommentEmpty, setIsCommentEmpty] = useState(true);
  return (
    <div>
      <AttachmentPicker />
      <ComposerWithSuggestions setIsCommentEmpty={setIsCommentEmpty} />
      <EmojiPickerButton />
      <SendButton disabled={isCommentEmpty} />
    </div>
  );
}
export const interaction = "type 5 chars";
export async function interact(ui: any) { await ui.type("#composer", "hello"); }
