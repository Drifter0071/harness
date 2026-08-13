import { useState } from "react";
import { ChevronDown, ChevronRight, Brain } from "lucide-react";
import { MessageBody } from "../lib/markdown";

interface MessageRendererProps {
  content: string;
  /** Chain-of-thought captured for thinking-capable models. When
   *  non-empty, renders a collapsible "Thinking" block above the
   *  message body. */
  reasoning?: string;
}

export function MessageRenderer({ content, reasoning }: MessageRendererProps) {
  const hasReasoning = !!reasoning && reasoning.length > 0;
  const [open, setOpen] = useState(false);
  if (!hasReasoning) {
    return <MessageBody content={content} />;
  }
  return (
    <div className="space-y-2">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1.5 text-[11px] text-white-50 hover:text-white transition-colors group select-none"
        aria-expanded={open}
        aria-label={open ? "Hide thinking" : "Show thinking"}
      >
        <Brain size={12} className="text-white-30 group-hover:text-white-50" />
        <span className="font-medium tracking-wide">Thinking</span>
        {open ? (
          <ChevronDown size={12} className="text-white-30" />
        ) : (
          <ChevronRight size={12} className="text-white-30" />
        )}
      </button>
      {open && (
        <div className="rounded border border-border-subtle bg-surface-2 px-3 py-2 text-[12px] text-white-50 leading-relaxed whitespace-pre-wrap animate-fade-in">
          {reasoning}
        </div>
      )}
      <MessageBody content={content} />
    </div>
  );
}
