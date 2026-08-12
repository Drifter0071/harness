// Model state bootstrap (Phase 2.0).
//
// Runs once at app mount:
//   1. One-time localStorage → keyring migration (spec §8). Full abort
//      on failure; the next launch retries.
//   2. Loads saved models + the sticky active model into the store.
//   3. Keeps modelPrefs in sync whenever the active model changes.

import { useEffect } from "react";
import { useStore } from "./store";
import { ipc } from "./ipc";
import { migrateLocalStorageEntries } from "./models";
import { showToast } from "./toast";

export function useModelBootstrap(): void {
  useEffect(() => {
    let cancelled = false;

    (async () => {
      // 1. Migration (only runs when old localStorage entries exist).
      try {
        await migrateLocalStorageEntries();
      } catch (err) {
        if (!cancelled) {
          showToast("Could not migrate saved models. Please try again from Settings.");
          console.error("[harness] model migration failed:", err);
        }
        return; // abort — localStorage preserved, retry next launch
      }

      if (cancelled) return;

      // 2. Load saved models + active model.
      try {
        const [models, activeId] = await Promise.all([
          ipc.listSavedModels(),
          ipc.getActiveModel(),
        ]);
        if (cancelled) return;
        useStore.getState().setSavedModels(models);
        useStore.getState().setActiveModelId(activeId);
      } catch (err) {
        if (!cancelled) {
          showToast(`Failed to load saved models: ${err}`);
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  // 3. Prefs follow the active model.
  useEffect(() => {
    const unsub = useStore.subscribe((state, prev) => {
      if (state.activeModelId === prev.activeModelId) return;
      const id = state.activeModelId;
      if (!id) {
        useStore.getState().setModelPrefs({ thinkingOn: false, reasoningEffort: "medium" });
        return;
      }
      ipc
        .getModelPrefs(id)
        .then((prefs) => {
          // Only apply if the active model hasn't moved on.
          if (useStore.getState().activeModelId === id) {
            useStore.getState().setModelPrefs(prefs);
          }
        })
        .catch(() => {});
    });
    return unsub;
  }, []);
}
