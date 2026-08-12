// Lightbox client. Wires click handlers on every `button.lightbox-trigger`
// in the document; the buttons are produced by the markdown post-processor.

import { useEffect, useState } from "react";

export function LightboxClient() {
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    const onClick = (e: MouseEvent) => {
      const target = e.target as HTMLElement | null;
      const trigger = target?.closest<HTMLElement>("button.lightbox-trigger");
      if (!trigger) return;
      e.preventDefault();
      const dataSrc = trigger.dataset.src;
      if (dataSrc) setSrc(dataSrc);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setSrc(null);
    };
    document.addEventListener("click", onClick);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("click", onClick);
      document.removeEventListener("keydown", onKey);
    };
  }, []);

  if (!src) return null;
  return (
    <div
      className="fixed inset-0 z-[100] flex items-center justify-center bg-black/60 backdrop-blur-sm animate-fade-in"
      onClick={() => setSrc(null)}
      role="dialog"
      aria-label="Image preview"
    >
      <img
        src={src}
        alt=""
        className="max-w-[90vw] max-h-[85vh] object-contain"
        onClick={(e) => e.stopPropagation()}
      />
    </div>
  );
}
