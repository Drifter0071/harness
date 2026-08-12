// Mermaid block renderer. Renders the diagram to SVG in place.

import { useEffect, useRef, useState } from "react";
import mermaid from "mermaid";

let counter = 0;

export function MermaidBlock({ source }: { source: string }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!containerRef.current) return;
    const id = `mermaid-${++counter}`;
    let cancelled = false;
    (async () => {
      try {
        const { svg } = await mermaid.render(id, source.trim());
        if (cancelled || !containerRef.current) return;
        containerRef.current.innerHTML = svg;
        setError(null);
      } catch (e) {
        if (cancelled) return;
        setError((e as Error).message);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [source]);

  if (error) {
    return (
      <pre className="border border-status-error/40 bg-status-error/5 rounded p-3 text-[12px] text-status-error font-mono whitespace-pre-wrap">
        Mermaid error: {error}
      </pre>
    );
  }
  return (
    <div
      ref={containerRef}
      className="mermaid-block my-2 flex justify-center"
    />
  );
}
