import { useState, useRef, useEffect } from "react";
import { useStore } from "../lib/store";
import { ipc } from "../lib/ipc";
import { MessageRenderer } from "./MessageRenderer";
import type { CanvasPanel, SessionMessage } from "../lib/store";
import { nanoid } from "nanoid";
import { Send, X, Circle } from "lucide-react";

const nowIso = () => new Date().toISOString();

export function DynamicCanvas() {
  const { canvasPanels, removeCanvasPanel } = useStore();
  const count = canvasPanels.length;
  const gridCols = count <= 1 ? 1 : count <= 2 ? 2 : count <= 4 ? 2 : 3;

  return (
    <div
      className="flex-1 overflow-hidden p-1.5"
      style={{ display: "grid", gridTemplateColumns: `repeat(${gridCols}, 1fr)`, gridAutoRows: "1fr", gap: "4px" }}
    >
      {canvasPanels.map((panel) => (
        <SubCanvas key={panel.id} panel={panel} onClose={() => removeCanvasPanel(panel.id)} />
      ))}
    </div>
  );
}

function SubCanvas({ panel, onClose }: { panel: CanvasPanel; onClose: () => void }) {
  const [input, setInput] = useState("");
  const [messages, setMessages] = useState<SessionMessage[]>(panel.messages);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const { updateCanvasPanel } = useStore();

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  const handleSend = async () => {
    if (!input.trim()) return;
    const content = input.trim();
    setInput("");
    setMessages((prev) => [...prev, { id: nanoid(), role: "user", content, timestamp: nowIso() }]);
    updateCanvasPanel(panel.id, { status: "working" });
    try {
      const response = await ipc.sendAgentMessage(panel.agentId, content);
      setMessages((prev) => [...prev, { id: nanoid(), role: "assistant", content: response.content, timestamp: nowIso(), agentName: panel.agentName }]);
    } catch (err) {
      setMessages((prev) => [...prev, { id: nanoid(), role: "system", content: `Error: ${String(err)}`, timestamp: nowIso() }]);
    } finally {
      updateCanvasPanel(panel.id, { status: "idle" });
    }
  };

  return (
    <div className="flex flex-col bg-surface-1 rounded border border-border-subtle overflow-hidden">
      <div className="flex items-center justify-between px-3 py-1.5 border-b border-border-subtle bg-surface-2/50">
        <div className="flex items-center gap-2 min-w-0">
          <Circle size={6} className={
            panel.status === "working" ? "fill-status-active text-status-active animate-pulse" :
            panel.status === "error" ? "fill-status-error text-status-error" : "fill-white-15 text-white-15"
          } />
          <span className="text-[12px] font-medium text-white truncate">{panel.agentName}</span>
          <span className="text-[11px] text-white-15 truncate">{panel.role}</span>
        </div>
        <button
          onClick={onClose}
          className="glass-quartz glass-quartz--pill flex items-center justify-center w-7 h-7 text-white-50 hover:text-white shrink-0"
          title={`Close ${panel.agentName}`}
          aria-label={`Close ${panel.agentName}`}
        >
          <X size={13} />
        </button>
      </div>

      {messages.length === 0 ? (
        <div className="flex-1 flex items-center justify-center px-3">
          <span className="text-[12px] text-white-15 select-none">
            Message {panel.agentName} to start a thread
          </span>
        </div>
      ) : (
        <div className="flex-1 overflow-y-auto p-3 space-y-2.5">
          {messages.map((msg) => (
            <div key={msg.id} className="text-[13px]">
              {msg.role === "user" ? (
                <div className="text-white-50">{msg.content}</div>
              ) : (
                <div className="text-white"><MessageRenderer content={msg.content} reasoning={msg.reasoning} /></div>
              )}
            </div>
          ))}
          <div ref={messagesEndRef} />
        </div>
      )}

      <div className="border-t border-border-subtle p-2">
        <div className="flex items-center gap-1.5 bg-surface-2 rounded border border-border-subtle focus-within:border-border-strong transition-colors">
          <input
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleSend()}
            placeholder={`Message ${panel.agentName}...`}
            className="flex-1 bg-transparent text-[12px] text-white px-3 py-2 outline-none placeholder:text-white-30"
          />
          <button
            onClick={handleSend}
            disabled={!input.trim()}
            className="glass-quartz glass-quartz--pill shrink-0 w-7 h-7 mr-1 flex items-center justify-center text-white"
            title="Send"
            aria-label="Send"
          >
            <Send size={13} />
          </button>
        </div>
      </div>
    </div>
  );
}
