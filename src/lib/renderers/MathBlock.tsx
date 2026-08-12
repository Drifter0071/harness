// Math block renderer (```math ... ``` fenced code).
// Receives a katex module (loaded lazily) and returns a React component.

import { useMemo } from "react";
import type * as Katex from "katex";

export function MathBlock(katex: typeof Katex) {
  return function MathBlockInner({ source }: { source: string }) {
    const html = useMemo(() => {
      try {
        return katex.renderToString(source, {
          displayMode: true,
          throwOnError: false,
        });
      } catch (e) {
        return `<pre>${(e as Error).message}</pre>`;
      }
    }, [source]);
    return (
      <div
        className="katex-block my-2"
        dangerouslySetInnerHTML={{ __html: html }}
      />
    );
  };
}
