import { useEffect, useRef } from "react";

export function ReadingText({ text, start, end }: {
  text: string; start: number | null; end: number | null;
}) {
  const container = useRef<HTMLDivElement>(null);
  const word = useRef<HTMLElement>(null);
  const valid = start !== null && end !== null && start >= 0 && end > start && end <= text.length;

  useEffect(() => {
    const box = container.current;
    const mark = word.current;
    if (!box || !mark) return;
    const bounds = box.getBoundingClientRect();
    const target = mark.getBoundingClientRect();
    if (target.top < bounds.top + 12) box.scrollTop -= bounds.top + 12 - target.top;
    else if (target.bottom > bounds.bottom - 12) box.scrollTop += target.bottom - bounds.bottom + 12;
  }, [text, start, end]);

  return <div className="selection-box" ref={container}>
    {valid ? <>{text.slice(0, start)}<mark className="spoken-word" ref={word}>
      {text.slice(start, end)}
    </mark>{text.slice(end)}</> : text}
  </div>;
}
