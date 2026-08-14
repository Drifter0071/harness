// Bip — the harness agent mascot.
//
// A chibi robot HEAD: the angular helmet, side fins and mouth grille
// of a Transformer, mixed with the Eva Unit-01 signature (bright
// violet + neon lime, the horn, the nose-crest visor) — all rounded
// and cartoon-cute. The SVG body is static per mood; every bit of
// motion is CSS-driven (see the "Bip" section of index.css) so it
// stays cheap and honours prefers-reduced-motion.
//
// This is the one sanctioned non-Lucide illustration in the app — it
// is a *character*, not an icon, so it deliberately breaks the
// "Lucide icons only" rule. Standard UI icons stay Lucide.
//
// Exports:
//   AgentMascot  — the full mascot. `mood` picks the expression:
//                  idle (dozing) · working (big alert eyes + orbiting
//                  data bits) · success (happy arcs) · error (X eyes).
//   ConfettiBurst— the success confetti, absolutely positioned over
//                  the mascot's wrapper (wrapper must be `relative`).
//   TypingBuddy  — the tiny Bip used in place of a plain blinking
//                  cursor: it "types" text into the stream, or
//                  sweeps it up while the agent stream-deletes.

import { useId, type CSSProperties } from "react";

export type MascotMood = "idle" | "working" | "success" | "error";
export type BuddyMode = "typing" | "sweeping";

/** Eva Unit-01 palette — bright cartoon violet body, neon lime accents. */
const EVA = {
  bodyTop: "#9d6bff", // bright violet
  bodyBottom: "#7c3aed", // violet-600 — shading, still vivid
  plate: "#8b5cf6", // fins / collar
  plateDark: "#6d28d9", // brow / recesses
  visor: "#0b0b16", // deep faceplate for contrast
  bone: "#ffffff", // horn
  idle: "#9de96e", // soft lime — dozing
  working: "#a3e635", // neon lime — alert
  success: "#ffc94d", // sun — celebration
  error: "#ff6b5e", // coral — hmm
};

/** Accent hue per mood (horn tip, visor crest, eyes). */
const HUE: Record<MascotMood, string> = {
  idle: EVA.idle,
  working: EVA.working,
  success: EVA.success,
  error: EVA.error,
};

const CONFETTI_COLORS = ["#a3e635", "#8b5cf6", "#ffc94d", "#b28dff", "#54e3a8"];

export function AgentMascot({
  mood,
  size = 30,
  className,
}: {
  mood: MascotMood;
  size?: number;
  className?: string;
}) {
  const uid = useId().replace(/[^a-zA-Z0-9]/g, "");
  const hue = HUE[mood];
  const blinkable = mood === "idle" || mood === "working";

  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 96 96"
      className={className}
      aria-hidden="true"
    >
      <defs>
        <linearGradient id={`bip-body-${uid}`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor={EVA.bodyTop} />
          <stop offset="100%" stopColor={EVA.bodyBottom} />
        </linearGradient>
      </defs>

      {/* Head fins — the angular Transformer "ears", neon lime outer edge */}
      <path d="M23 33 L8 39 L11 57 L24 49 Z" fill={EVA.plate} />
      <path d="M8 39 L11 57" stroke={hue} strokeWidth="2" strokeLinecap="round" fill="none" opacity="0.9" />
      <path d="M73 33 L88 39 L85 57 L72 49 Z" fill={EVA.plate} />
      <path d="M88 39 L85 57" stroke={hue} strokeWidth="2" strokeLinecap="round" fill="none" opacity="0.9" />

      {/* The horn — Eva Unit-01 signature, white bone, glowing lime tip */}
      <path d="M48 24 C 47 17, 45 12, 41.5 8 C 47 10, 50 15, 51 23 Z" fill={EVA.bone} />
      <circle cx="41.5" cy="8" r="2.6" fill={hue} className="mascot-glow" />

      {/* Head — big rounded helmet */}
      <rect x="17" y="23" width="62" height="50" rx="20" fill={`url(#bip-body-${uid})`} />

      {/* Brow plate — layered Transformer helmet ridge */}
      <rect x="25" y="33" width="46" height="7" rx="3.5" fill={EVA.plateDark} />

      {/* Visor — rounded Eva faceplate */}
      <rect x="30" y="40" width="36" height="18" rx="9" fill={EVA.visor} />
      {/* The Unit-01 nose crest line */}
      <rect x="47.2" y="40" width="1.8" height="17" fill={hue} opacity="0.9" />

      {/* Eyes — big cartoon eyes in the visor */}
      <g className={blinkable ? "mascot-eyes" : undefined}>
        {mood === "success" ? (
          // Happy arcs
          <>
            <path d="M37 50 Q41 54.5 45 50" stroke={hue} strokeWidth="2.6" strokeLinecap="round" fill="none" />
            <path d="M51 50 Q55 54.5 59 50" stroke={hue} strokeWidth="2.6" strokeLinecap="round" fill="none" />
          </>
        ) : mood === "error" ? (
          // X eyes
          <>
            <path d="M37.5 46.5 L44.5 52.5 M44.5 46.5 L37.5 52.5" stroke={hue} strokeWidth="2.4" strokeLinecap="round" />
            <path d="M51.5 46.5 L58.5 52.5 M58.5 46.5 L51.5 52.5" stroke={hue} strokeWidth="2.4" strokeLinecap="round" />
          </>
        ) : mood === "working" ? (
          // Wide-open glossy eyes — alert
          <>
            <circle cx="41" cy="49" r="4.6" fill={hue} />
            <circle cx="55" cy="49" r="4.6" fill={hue} />
            <circle cx="39.6" cy="47.4" r="1.5" fill={EVA.visor} />
            <circle cx="53.6" cy="47.4" r="1.5" fill={EVA.visor} />
          </>
        ) : (
          // Idle — dozing closed lids
          <>
            <path d="M37 49 Q41 45 45 49" stroke={hue} strokeWidth="2.4" strokeLinecap="round" fill="none" opacity="0.8" />
            <path d="M51 49 Q55 45 59 49" stroke={hue} strokeWidth="2.4" strokeLinecap="round" fill="none" opacity="0.8" />
          </>
        )}
      </g>

      {/* Mouth grille — tiny Transformer vent */}
      <g opacity="0.85">
        <rect x="42.5" y="61" width="2" height="5" rx="1" fill={hue} />
        <rect x="47" y="61" width="2" height="5" rx="1" fill={hue} />
        <rect x="51.5" y="61" width="2" height="5" rx="1" fill={hue} />
      </g>

      {/* Chest collar — small pauldron nubs + glowing core */}
      <path d="M20 74 L12 80 L14 90 L24 85 Z" fill={EVA.plate} />
      <path d="M12 80 L14 90" stroke={hue} strokeWidth="1.8" strokeLinecap="round" fill="none" opacity="0.9" />
      <path d="M76 74 L84 80 L82 90 L72 85 Z" fill={EVA.plate} />
      <path d="M84 80 L82 90" stroke={hue} strokeWidth="1.8" strokeLinecap="round" fill="none" opacity="0.9" />
      <circle cx="48" cy="83" r="3.6" fill={hue} className="mascot-glow" />

      {/* Working — orbiting data bits */}
      {mood === "working" && (
        <g className="mascot-orbit">
          <circle cx="14" cy="26" r="3.2" fill={EVA.working} />
          <circle cx="82" cy="44" r="3.2" fill="#ffc94d" />
          <circle cx="20" cy="68" r="3.2" fill="#b28dff" />
        </g>
      )}
    </svg>
  );
}

/** Success confetti — absolutely positioned over the mascot's
 *  `relative` wrapper; pieces fly outward from the mascot's center. */
export function ConfettiBurst() {
  const pieces = [
    { tx: -18, ty: -24, rot: 200, color: CONFETTI_COLORS[0], delay: 0 },
    { tx: 16, ty: -28, rot: -160, color: CONFETTI_COLORS[2], delay: 0.05 },
    { tx: 26, ty: -8, rot: 120, color: CONFETTI_COLORS[4], delay: 0.1 },
    { tx: -26, ty: -6, rot: -120, color: CONFETTI_COLORS[3], delay: 0.15 },
    { tx: 20, ty: 18, rot: 90, color: CONFETTI_COLORS[1], delay: 0.2 },
    { tx: -20, ty: 14, rot: -80, color: CONFETTI_COLORS[2], delay: 0.25 },
    { tx: 0, ty: -32, rot: 300, color: CONFETTI_COLORS[0], delay: 0.1 },
    { tx: -10, ty: 24, rot: -240, color: CONFETTI_COLORS[4], delay: 0.3 },
    { tx: 32, ty: -2, rot: 160, color: CONFETTI_COLORS[3], delay: 0.18 },
    { tx: -32, ty: 4, rot: -140, color: CONFETTI_COLORS[1], delay: 0.28 },
  ] as const;
  return (
    <>
      {pieces.map((p, i) => (
        <span
          key={i}
          className="confetti-piece"
          style={{
            background: p.color,
            // Cast: CSS custom props are untyped by design.
            ...({ "--tx": `${p.tx}px`, "--ty": `${p.ty}px`, "--rot": `${p.rot}deg` } as CSSProperties),
            animationDelay: `${p.delay}s`,
          }}
        />
      ))}
    </>
  );
}

/** The tiny Bip that stands in for a plain blinking cursor.
 *  `typing` — bouncing while text streams in.
 *  `sweeping` — brandishes a tiny broom while the agent stream-deletes. */
export function TypingBuddy({
  mode = "typing",
  size = 16,
  className,
}: {
  mode?: BuddyMode;
  size?: number;
  className?: string;
}) {
  const uid = useId().replace(/[^a-zA-Z0-9]/g, "");
  return (
    <span
      className={`inline-flex items-center justify-center ${mode === "typing" ? "mascot-typing" : "mascot-sweep"} ${className ?? ""}`}
      aria-hidden="true"
      title={mode === "typing" ? "typing…" : "sweeping…"}
    >
      <svg width={size} height={size} viewBox="0 0 64 64">
        <defs>
          <linearGradient id={`bip-buddy-${uid}`} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={EVA.bodyTop} />
            <stop offset="100%" stopColor={EVA.bodyBottom} />
          </linearGradient>
        </defs>

        {/* Horn */}
        <path d="M32 17 L30.5 11 L33.5 11.5 Z" fill={EVA.bone} />
        <circle cx="30.8" cy="11.2" r="1.5" fill={EVA.working} className="mascot-glow" />

        {/* Head — rounded helmet */}
        <rect x="17" y="16" width="30" height="28" rx="11" fill={`url(#bip-buddy-${uid})`} />

        {/* Brow plate */}
        <rect x="20" y="20" width="24" height="4" rx="2" fill={EVA.plateDark} />

        {/* Visor */}
        <rect x="21" y="24" width="22" height="11" rx="5.5" fill={EVA.visor} />
        <rect x="31.4" y="24" width="1.2" height="10" fill={EVA.working} opacity="0.9" />

        {/* Eyes */}
        {mode === "sweeping" ? (
          // Squint — focused on the mess
          <>
            <path d="M25 30.5 Q26.5 28.8 28 30.5" stroke={EVA.working} strokeWidth="1.6" strokeLinecap="round" fill="none" />
            <path d="M36 30.5 Q37.5 28.8 39 30.5" stroke={EVA.working} strokeWidth="1.6" strokeLinecap="round" fill="none" />
          </>
        ) : (
          // Bright round eyes with glossy highlights
          <>
            <circle cx="26.8" cy="29.5" r="1.9" fill={EVA.working} />
            <circle cx="37.2" cy="29.5" r="1.9" fill={EVA.working} />
            <circle cx="26.2" cy="29" r="0.6" fill={EVA.visor} />
            <circle cx="36.6" cy="29" r="0.6" fill={EVA.visor} />
          </>
        )}

        {/* Mouth grille */}
        <g opacity="0.85">
          <rect x="29" y="37.5" width="1.2" height="3" rx="0.6" fill={EVA.working} />
          <rect x="31.4" y="37.5" width="1.2" height="3" rx="0.6" fill={EVA.working} />
          <rect x="33.8" y="37.5" width="1.2" height="3" rx="0.6" fill={EVA.working} />
        </g>

        {/* Chest core */}
        <circle cx="32" cy="43" r="1.8" fill={EVA.working} className="mascot-glow" />

        {mode === "sweeping" && (
          <>
            {/* Broom — handle + head, wiggling */}
            <g className="broom-wiggle">
              <line x1="38" y1="52" x2="48" y2="41" stroke="#d7d7de" strokeWidth="2" strokeLinecap="round" />
              <path d="M46 39 L52 33 L56 37 L50 43 Z" fill="#ff6b5e" />
            </g>
            {/* Dust specks flying off the swept text */}
            <circle className="sweep-dust" cx="26" cy="44" r="1.2" fill="#ffc94d" style={{ "--dx": "-5px", "--dy": "-4px" } as CSSProperties} />
            <circle className="sweep-dust" cx="22" cy="38" r="1" fill="#5bc9ff" style={{ "--dx": "-6px", "--dy": "-3px", animationDelay: "0.25s" } as CSSProperties} />
            <circle className="sweep-dust" cx="29" cy="50" r="1" fill="#b28dff" style={{ "--dx": "-7px", "--dy": "2px", animationDelay: "0.12s" } as CSSProperties} />
          </>
        )}
      </svg>
    </span>
  );
}
