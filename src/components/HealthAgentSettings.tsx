// HealthAgentSettings — Settings panel section for the Health Agent.
//
// v0 keeps this self-contained: persists to localStorage under
// `harness_health_config` so we don't need a Tauri command for it.
// The Rust side reads its own defaults from env; the UI values are
// forwarded to the Rust Health Agent via a follow-up IPC once we
// wire the Tauri command (deferred — see spec §4.4).

import { useEffect, useState } from "react";
import { useStore } from "../lib/store";
import { ipc } from "../lib/ipc";

const STORAGE_KEY = "harness_health_config";

export interface HealthConfig {
  enabled: boolean;
  modelId: string | null;
  tickIntervalSecs: number;
  perTickMaxMs: number;
}

const DEFAULTS: HealthConfig = {
  enabled: false, // OFF in release by default; Settings flips it on.
  modelId: null,
  tickIntervalSecs: 30,
  perTickMaxMs: 20000,
};

function loadFromStorage(): HealthConfig {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULTS };
    const parsed = JSON.parse(raw) as Partial<HealthConfig>;
    return {
      enabled: parsed.enabled ?? DEFAULTS.enabled,
      modelId: parsed.modelId ?? DEFAULTS.modelId,
      tickIntervalSecs: clamp(
        parsed.tickIntervalSecs ?? DEFAULTS.tickIntervalSecs,
        10,
        300,
      ),
      perTickMaxMs: clamp(
        parsed.perTickMaxMs ?? DEFAULTS.perTickMaxMs,
        10_000,
        60_000,
      ),
    };
  } catch {
    return { ...DEFAULTS };
  }
}

function saveToStorage(cfg: HealthConfig): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(cfg));
  } catch {
    // localStorage may be full or disabled (private mode). Best-effort.
  }
}

function clamp(n: number, min: number, max: number): number {
  if (Number.isNaN(n)) return min;
  return Math.max(min, Math.min(max, n));
}

export function HealthAgentSettings() {
  const models = useStore((s) => s.savedModels);
  const [cfg, setCfg] = useState<HealthConfig>(() => loadFromStorage());
  const [savedHint, setSavedHint] = useState<string>("");

  useEffect(() => {
    saveToStorage(cfg);
    setSavedHint("Saved");
    const t = setTimeout(() => setSavedHint(""), 1200);
    return () => clearTimeout(t);
  }, [cfg]);

  // Forward to the Rust side. Tauri command is deferred; for v0 the
  // Rust side reads from env only. Hook the placeholder so the wiring
  // is documented.
  useEffect(() => {
    // TODO(wp10-followup): call `ipc.healthAgentUpdateConfig(cfg)`.
    // The Rust command is not yet registered; the UI persists locally
    // and the Rust side uses its own default until the bridge lands.
  }, [cfg]);

  return (
    <section className="space-y-3 px-1">
      <div>
        <h3 className="text-[13px] text-white font-medium">Health Agent</h3>
        <p className="text-[11px] text-white-15 mt-1">
          Periodic meta-agent that watches the active session and can
          intervene when a sub-agent gets stuck. Spawns on a tick, runs
          for a bounded time, terminates.
        </p>
      </div>

      <label className="flex items-center gap-2 text-[12px] text-white-50">
        <input
          type="checkbox"
          checked={cfg.enabled}
          onChange={(e) => setCfg({ ...cfg, enabled: e.target.checked })}
          className="accent-white"
        />
        Enable Health Agent
      </label>

      <div className="space-y-1">
        <label className="text-[11px] text-white-15 uppercase tracking-[0.14em]">
          Model
        </label>
        <select
          value={cfg.modelId ?? ""}
          onChange={(e) =>
            setCfg({ ...cfg, modelId: e.target.value || null })
          }
          className="w-full px-3 py-2 bg-surface-2 border border-border-subtle rounded text-[13px] text-white outline-none focus:border-border-strong"
        >
          <option value="">Use chat model</option>
          {models.map((m) => (
            <option key={m.id} value={m.id}>
              {m.label} ({m.modelName})
            </option>
          ))}
        </select>
      </div>

      <div className="space-y-1">
        <div className="flex items-center justify-between">
          <label className="text-[11px] text-white-15 uppercase tracking-[0.14em]">
            Tick interval
          </label>
          <span className="text-[11px] text-white-30 font-mono">
            {cfg.tickIntervalSecs}s
          </span>
        </div>
        <input
          type="range"
          min={10}
          max={300}
          step={5}
          value={cfg.tickIntervalSecs}
          onChange={(e) =>
            setCfg({ ...cfg, tickIntervalSecs: Number(e.target.value) })
          }
          className="w-full"
        />
        <p className="text-[10px] text-white-15">10s – 5min</p>
      </div>

      <div className="space-y-1">
        <div className="flex items-center justify-between">
          <label className="text-[11px] text-white-15 uppercase tracking-[0.14em]">
            Per-tick duration
          </label>
          <span className="text-[11px] text-white-30 font-mono">
            {(cfg.perTickMaxMs / 1000).toFixed(1)}s
          </span>
        </div>
        <input
          type="range"
          min={10000}
          max={60000}
          step={1000}
          value={cfg.perTickMaxMs}
          onChange={(e) =>
            setCfg({ ...cfg, perTickMaxMs: Number(e.target.value) })
          }
          className="w-full"
        />
        <p className="text-[10px] text-white-15">10s – 60s (hard timeout)</p>
      </div>

      {savedHint && (
        <p className="text-[10px] text-status-active font-mono">{savedHint}</p>
      )}
    </section>
  );
}
