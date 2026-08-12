// Tiny non-blocking toast surface. Reuses the existing visual language
// (surface-2 / border-subtle / white-50). No new deps.

import { useEffect, useState } from "react";
import { useStore } from "./store";

interface ToastItem {
  id: number;
  text: string;
}

let nextId = 1;
let setToastsExternal: ((items: ToastItem[]) => void) | null = null;
let current: ToastItem[] = [];

export function showToast(text: string): void {
  const item: ToastItem = { id: nextId++, text };
  current = [...current, item];
  setToastsExternal?.(current);
  setTimeout(() => {
    current = current.filter((t) => t.id !== item.id);
    setToastsExternal?.(current);
  }, 4500);
}

export function ToastHost() {
  const [items, setItems] = useState<ToastItem[]>([]);

  useEffect(() => {
    setToastsExternal = setItems;
    setItems(current);
    return () => {
      setToastsExternal = null;
    };
  }, []);

  if (items.length === 0) return null;
  return (
    <div className="fixed bottom-4 right-4 z-50 space-y-2 max-w-[360px]">
      {items.map((t) => (
        <div
          key={t.id}
          className="bg-surface-2 border border-border-subtle rounded px-3 py-2 text-[12px] text-white-50 shadow-lg animate-fade-in"
        >
          {t.text}
        </div>
      ))}
    </div>
  );
}
