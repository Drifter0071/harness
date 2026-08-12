// Inline math renderer ($...$).

import { useMemo } from "react";
import type * as Katex from "katex";

export function InlineMath(katex: typeof Katex) {
  return function InlineMathInner({ source }: { source: string }) {
    const html = useMemo(() => {
      try {
        return katex.renderToString(source, {
          displayMode: false,
          throwOnError: false,
        });
      } catch (e) {
        return `<code>${(e as Error).message}</code>`;
      }
    }, [source]);
    return (
      <span
        className="katex-inline"
        dangerouslySetInnerHTML={{ __html: html }}
      />
    );
  };
}
