// Bip — the harness agent mascot.
//
// A chibi robot HEAD: the angular helmet, side fins and signature
// horn of an Eva Unit-01 / Transformer cross, all rounded and
// cartoon-cute. The SVG is a *persistent* element tree — eyes, mouth
// and horn tip are single paths that GSAP morphs between expression
// shapes so mood transitions feel like a single character changing
// expression, not a costume swap. The body color and accent hue
// also shift with mood (sky / lilac / sun / coral) so the whole
// character "re-tints" rather than just the eyes.
//
// Motion vocabulary (see `index.css` for the long-form):
//   • breath           — single 3.6s source-of-truth cycle everything locks to
//   • bob / bob-fast   — body float, idle vs working
//   • celebrate        — one-shot anticipation→launch→land→settle
//   • shake            — small tremble for error
//   • fin / horn / core — secondary motion on the head & chest
//   • blink            — eye scaleY pulse
//   • sparkle bits     — fade in → orbit → fade out, staggered
//   • entrance         — first-mount only, 480ms reveal
//
// Honors `prefers-reduced-motion` via the `bip-reduced` class on the
// root SVG, set at construction time — when set, GSAP skips and the
// eyes fall back to the neutral open shape with a static mid-glow.
//
// This is the one sanctioned non-Lucide illustration in the app —
// it is a *character*, not an icon, so it deliberately breaks the
// "Lucide icons only" rule. Standard UI icons stay Lucide.
//
// Exports:
//   AgentMascot  — the full mascot. `mood` picks the expression:
//                  idle · working · success · error.
//   ConfettiBurst— the success confetti, absolutely positioned over
//                  the mascot's wrapper (wrapper must be `relative`).
//   TypingBuddy  — the tiny Bip used in place of a plain blinking
//                  cursor: it "types" text into the stream, or
//                  sweeps it up while the agent stream-deletes.

import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  type CSSProperties,
} from "react";
import { gsap } from "gsap";
import { MorphSVGPlugin } from "gsap/MorphSVGPlugin";

gsap.registerPlugin(MorphSVGPlugin);

export type MascotMood = "idle" | "working" | "success" | "error";
export type BuddyMode = "typing" | "sweeping";

/* ──────────────────────────────────────────────────────────────────
 * Mood palette — sky / lilac / sun / coral, lifted from the app's
 * own accent palette so Bip belongs to the same world as the rest
 * of the chrome. The body *tints* to the mood's color, the accent
 * (horn tip, eye, chest core) keeps a complementary punch.
 * ────────────────────────────────────────────────────────────────── */
const MOOD = {
  idle: {
    body: ["#5bc9ff", "#2b8fc7"],     // sky → deeper sky
    plate: "#3aa3d6",
    plateDark: "#1f6f9a",
    accent: "#b28dff",                 // lilac — calm twinkle
    halo: "rgba(91, 201, 255, 0.30)",  // sky
  },
  working: {
    body: ["#b28dff", "#7c3aed"],     // lilac → violet
    plate: "#8b5cf6",
    plateDark: "#6d28d9",
    accent: "#54e3a8",                 // mint — focused spark
    halo: "rgba(178, 141, 255, 0.32)", // lilac
  },
  success: {
    body: ["#ffc94d", "#e0a02a"],     // sun → amber
    plate: "#f0b73a",
    plateDark: "#b87a17",
    accent: "#54e3a8",                 // mint — celebration pop
    halo: "rgba(255, 201, 77, 0.34)",  // sun
  },
  error: {
    body: ["#ff6b5e", "#c93f33"],     // coral → red
    plate: "#d44b40",
    plateDark: "#8c2620",
    accent: "#b28dff",                 // lilac — softens the red
    halo: "rgba(239, 68, 68, 0.30)",   // red
  },
} as const;

const BONE = "#ffffff";
const VISOR = "#0b0b16";

const CONFETTI_COLORS = ["#a3e635", "#8b5cf6", "#ffc94d", "#b28dff", "#54e3a8"];

/* ──────────────────────────────────────────────────────────────────
 * Expression paths — every eye and the mouth are authored so they
 * share the same command structure and point count, which is what
 * GSAP MorphSVG needs to interpolate cleanly between them.
 *
 * Coordinate space: a 96×96 viewBox centered on the face.
 * Left eye center  ~ (40, 51)
 * Right eye center ~ (56, 51)
 * Mouth center     ~ (48, 64)
 * ────────────────────────────────────────────────────────────────── */
const EYE_OPEN = {
  left:  "M35 51 Q40 45.5 45 51 Q40 56.5 35 51 Z",
  right: "M51 51 Q56 45.5 61 51 Q56 56.5 51 51 Z",
};
const EYE_HALF = {
  left:  "M36 51 Q40 49 44 51 Q40 53 36 51 Z",
  right: "M52 51 Q56 49 60 51 Q56 53 52 51 Z",
};
const EYE_CLOSED = {
  left:  "M36 51 Q40 50.4 44 51",
  right: "M52 51 Q56 50.4 60 51",
};
const EYE_HAPPY = {
  left:  "M35 52 Q40 47 45 52",
  right: "M51 52 Q56 47 61 52",
};
const EYE_X = {
  left:  "M36 47 L44 55 M44 47 L36 55",
  right: "M52 47 L60 55 M60 47 L52 55",
};
const EYE_O = {
  left:  "M40 51 m-3 0 a3 3 0 1 0 6 0 a3 3 0 1 0 -6 0",
  right: "M56 51 m-3 0 a3 3 0 1 0 6 0 a3 3 0 1 0 -6 0",
};

/* Mouth — single path, four personalities */
const MOUTH_GRIN  = "M40 64 Q48 71 56 64";          // celebration smile
const MOUTH_FLAT  = "M41 65 L55 65";               // neutral, surprised-flat
const MOUTH_O     = "M48 65 m-2.4 0 a2.4 1.8 0 1 0 4.8 0 a2.4 1.8 0 1 0 -4.8 0"; // "o" surprise
const MOUTH_FROWN = "M40 67 Q48 63 56 67";          // tiny downturn for error
const MOUTH_NEUTRAL = "M43 65 L53 65";             // resting / idle

/* Pick the right expression for a mood. Eyes and mouth can be tuned
 * independently per mood — e.g. "working" is wide-open eyes with a
 * small flat mouth, "success" is happy arcs with a big smile. */
const EXPRESSION_FOR: Record<MascotMood, { eyes: { left: string; right: string }; mouth: string }> = {
  idle:    { eyes: EYE_HALF,   mouth: MOUTH_NEUTRAL },
  working: { eyes: EYE_OPEN,   mouth: MOUTH_FLAT },
  success: { eyes: EYE_HAPPY,  mouth: MOUTH_GRIN },
  error:   { eyes: EYE_X,      mouth: MOUTH_FROWN },
};

/* A "swept" expression for the buddy when it's stream-deleting —
 * squinty eyes, a slightly-open mouth. */
const SWEEP_EXPRESSION = { eyes: EYE_HALF, mouth: MOUTH_O };

const REDUCED = typeof window !== "undefined"
  && window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;

/* ──────────────────────────────────────────────────────────────────
 * AgentMascot — the main character.
 * ────────────────────────────────────────────────────────────────── */
export function AgentMascot({
  mood,
  size = 30,
  className,
  firstMount = false,
}: {
  mood: MascotMood;
  size?: number;
  className?: string;
  /** Set true on the very first render of this mascot instance —
   *  triggers the 480ms entrance reveal. Subsequent re-renders
   *  (mood changes, prop changes) skip the entrance. */
  firstMount?: boolean;
}) {
  const uid = useId().replace(/[^a-zA-Z0-9]/g, "");
  const m = MOOD[mood];

  // Refs to the parts GSAP will tween
  const rootRef   = useRef<SVGSVGElement>(null);
  const leftEye   = useRef<SVGPathElement>(null);
  const rightEye  = useRef<SVGPathElement>(null);
  const mouth     = useRef<SVGPathElement>(null);
  const bodyRef   = useRef<SVGRectElement>(null);
  const bodyGrad  = useRef<SVGLinearGradientElement>(null);
  const plateRef  = useRef<SVGRectElement>(null);
  const plateDarkRef = useRef<SVGRectElement>(null);
  const hornTip   = useRef<SVGCircleElement>(null);
  const chestCore = useRef<SVVCircleElement | null>(null);

  // Keep latest values in a ref so the morph effect doesn't re-bind
  const stateRef = useRef({ mood });
  stateRef.current.mood = mood;

  /* GSAP morph on mood change. Single timeline per change keeps the
   * eyes + mouth in sync and lets us share the same easing. */
  useLayoutEffect(() => {
    if (REDUCED) return;
    if (!leftEye.current || !rightEye.current || !mouth.current) return;
    const target = EXPRESSION_FOR[mood];
    const tl = gsap.timeline({ defaults: { duration: 0.32, ease: "power2.out" } });
    tl.to(leftEye.current,  { morphSVG: target.eyes.left,  attr: { fill: target.eyes === EYE_HAPPY || target.eyes === EYE_X ? "none" : m.accent } }, 0)
      .to(rightEye.current, { morphSVG: target.eyes.right, attr: { fill: target.eyes === EYE_HAPPY || target.eyes === EYE_X ? "none" : m.accent } }, 0)
      .to(mouth.current,    { morphSVG: target.mouth, attr: { stroke: m.accent } }, 0);
    return () => { tl.kill(); };
  }, [mood, m.accent]);

  /* Body color tween on mood change — animates the two gradient stops
   * so the whole head re-tints rather than just the accents. */
  useLayoutEffect(() => {
    if (REDUCED) return;
    if (!bodyGrad.current) return;
    const stops = bodyGrad.current.querySelectorAll("stop");
    if (stops.length < 2) return;
    const tl = gsap.timeline({ defaults: { duration: 0.45, ease: "power2.inOut" } });
    tl.to(stops[0], { stopColor: m.body[0] }, 0)
      .to(stops[1], { stopColor: m.body[1] }, 0)
      .to(plateRef.current,    { fill: m.plate }, 0)
      .to(plateDarkRef.current, { fill: m.plateDark }, 0)
      .to(hornTip.current,     { fill: m.accent }, 0)
      .to(chestCore.current,    { fill: m.accent }, 0);
    return () => { tl.kill(); };
  }, [mood, m.body, m.plate, m.plateDark, m.accent]);

  /* One-shot entrance on first mount. */
  useEffect(() => {
    if (!firstMount || REDUCED || !rootRef.current) return;
    const el = rootRef.current;
    gsap.fromTo(
      el,
      { opacity: 0, scale: 0.92, transformOrigin: "50% 90%" },
      { opacity: 1, scale: 1, duration: 0.48, ease: "power3.out" }
    );
    if (hornTip.current) {
      gsap.fromTo(
        hornTip.current,
        { scale: 0.2, transformOrigin: "50% 50%" },
        { scale: 1, duration: 0.6, ease: "elastic.out(1, 0.6)", delay: 0.15 }
      );
    }
  }, [firstMount]);

  return (
    <svg
      ref={rootRef}
      width={size}
      height={size}
      viewBox="0 0 96 96"
      className={className}
      data-mood={mood}
      data-bip="agent"
      aria-hidden="true"
    >
      <defs>
        <linearGradient
          ref={bodyGrad}
          id={`bip-body-${uid}`}
          x1="0"
          y1="0"
          x2="0"
          y2="1"
        >
          <stop offset="0%" stopColor={m.body[0]} />
          <stop offset="100%" stopColor={m.body[1]} />
        </linearGradient>
        <radialGradient id={`bip-cheek-${uid}`} cx="50%" cy="50%" r="50%">
          <stop offset="0%" stopColor={m.accent} stopOpacity="0.55" />
          <stop offset="100%" stopColor={m.accent} stopOpacity="0" />
        </radialGradient>
      </defs>

      {/* Head fins — gentle float, neon-lime outer edge */}
      <g className="bip-fin bip-fin--left">
        <path d="M23 33 L8 39 L11 57 L24 49 Z" fill={m.plate} />
        <path d="M8 39 L11 57" className="bip-fin-edge" stroke={m.accent} strokeWidth="2" strokeLinecap="round" fill="none" />
      </g>
      <g className="bip-fin bip-fin--right">
        <path d="M73 33 L88 39 L85 57 L72 49 Z" fill={m.plate} />
        <path d="M88 39 L85 57" className="bip-fin-edge" stroke={m.accent} strokeWidth="2" strokeLinecap="round" fill="none" />
      </g>

      {/* The horn — Eva signature, white bone, glowing tip */}
      <path d="M48 24 C 47 17, 45 12, 41.5 8 C 47 10, 50 15, 51 23 Z" fill={BONE} />
      <circle ref={hornTip} cx="41.5" cy="8" r="2.6" fill={m.accent} className="mascot-glow bip-horn-tip" />
      <circle cx="41.5" cy="8" r="6" fill={m.accent} className="bip-horn-aura" />

      {/* Head — big rounded helmet. CSS does the breath scale. */}
      <rect
        ref={bodyRef}
        className="bip-body"
        x="14"
        y="23"
        width="68"
        height="54"
        rx="22"
        fill={`url(#bip-body-${uid})`}
      />

      {/* Brow plate — subtle alert ridge lift when working */}
      <rect
        ref={plateDarkRef}
        className="bip-brow"
        x="22"
        y="33"
        width="52"
        height="7"
        rx="3.5"
        fill={m.plateDark}
      />

      {/* Visor — bigger than v1, more room for eyes + cheek dots */}
      <rect x="22" y="40" width="52" height="22" rx="11" fill={VISOR} />
      {/* The Unit-01 nose crest line — draws on when working */}
      <line
        className="bip-nose-crest"
        x1="48"
        y1="40"
        x2="48"
        y2="60"
        stroke={m.accent}
        strokeWidth="1.8"
        strokeLinecap="round"
        opacity="0.9"
      />

      {/* Cheek dots — Eva-01 signature, soft accent glow */}
      <circle cx="28" cy="58" r="3.6" fill={`url(#bip-cheek-${uid})`} className="bip-cheek bip-cheek--left" />
      <circle cx="68" cy="58" r="3.6" fill={`url(#bip-cheek-${uid})`} className="bip-cheek bip-cheek--right" />

      {/* Eyes — single path per side, GSAP morphs between expressions */}
      <g className="bip-eyes">
        <path
          ref={leftEye}
          className="bip-eye bip-eye--left"
          d={EXPRESSION_FOR[mood].eyes.left}
          fill={EXPRESSION_FOR[mood].eyes === EYE_HAPPY || EXPRESSION_FOR[mood].eyes === EYE_X ? "none" : m.accent}
          stroke={m.accent}
          strokeWidth="2.4"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
        <path
          ref={rightEye}
          className="bip-eye bip-eye--right"
          d={EXPRESSION_FOR[mood].eyes.right}
          fill={EXPRESSION_FOR[mood].eyes === EYE_HAPPY || EXPRESSION_FOR[mood].eyes === EYE_X ? "none" : m.accent}
          stroke={m.accent}
          strokeWidth="2.4"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
        {/* Glossy catch-lights, only meaningful when eyes are open */}
        {EXPRESSION_FOR[mood].eyes === EYE_OPEN && (
          <>
            <circle className="bip-catchlight bip-catchlight--left" cx="38.5" cy="49.4" r="1.2" fill={VISOR} />
            <circle className="bip-catchlight bip-catchlight--right" cx="54.5" cy="49.4" r="1.2" fill={VISOR} />
          </>
        )}
      </g>

      {/* Mouth — single path, GSAP morphs between expressions */}
      <path
        ref={mouth}
        className="bip-mouth"
        d={EXPRESSION_FOR[mood].mouth}
        stroke={m.accent}
        strokeWidth="2.4"
        strokeLinecap="round"
        strokeLinejoin="round"
        fill="none"
        opacity="0.95"
      />

      {/* Chest collar — small pauldron nubs + breathing core */}
      <path d="M20 78 L12 84 L14 94 L24 89 Z" fill={m.plate} />
      <path d="M12 84 L14 94" stroke={m.accent} strokeWidth="1.8" strokeLinecap="round" fill="none" opacity="0.9" />
      <path d="M76 78 L84 84 L82 94 L72 89 Z" fill={m.plate} />
      <path d="M84 84 L82 94" stroke={m.accent} strokeWidth="1.8" strokeLinecap="round" fill="none" opacity="0.9" />
      <circle
        ref={(el) => { chestCore.current = el; }}
        cx="48"
        cy="87"
        r="3.8"
        fill={m.accent}
        className="mascot-glow bip-chest-core"
      />
      <circle cx="48" cy="87" r="7" fill={m.accent} className="bip-chest-aura" />

      {/* Sparkle orbit — only in working mood. The whole frame rotates
          slowly (7.2s = two breath cycles), and each individual bit
          pulses on its own period so the field looks alive instead of
          rigid. Each bit carries the mood's accent color so the
          sparkles harmonize with the body tint. */}
      {mood === "working" && (
        <g className="bip-orbit-frame">
          <circle
            className="bip-sparkle bip-sparkle--1"
            cx="6"  cy="48" r="2.6"
            fill={m.accent}
          />
          <circle
            className="bip-sparkle bip-sparkle--2"
            cx="90" cy="32" r="2.2"
            fill="#ffc94d"
          />
          <circle
            className="bip-sparkle bip-sparkle--3"
            cx="90" cy="64" r="2.4"
            fill="#54e3a8"
          />
          <circle
            className="bip-sparkle bip-sparkle--4"
            cx="6"  cy="32" r="2.0"
            fill="#5bc9ff"
          />
        </g>
      )}
    </svg>
  );
}

/* A tiny shape used for the SVGRectElement ref typing above */
type SVVCircleElement = SVGCircleElement;

/* ──────────────────────────────────────────────────────────────────
 * ConfettiBurst — success confetti (unchanged shape, CSS handles
 * the gravity arc in index.css).
 * ────────────────────────────────────────────────────────────────── */
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
            ...({ "--tx": `${p.tx}px`, "--ty": `${p.ty}px`, "--rot": `${p.rot}deg` } as CSSProperties),
            animationDelay: `${p.delay}s`,
          }}
        />
      ))}
    </>
  );
}

/* ──────────────────────────────────────────────────────────────────
 * TypingBuddy — the tiny Bip that "types" text into the stream,
 * or sweeps it up while the agent stream-deletes.
 * ────────────────────────────────────────────────────────────────── */
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
  const m = MOOD[mode === "sweeping" ? "working" : "working"];
  const sweep = mode === "sweeping";
  return (
    <span
      className={`inline-flex items-center justify-center ${mode === "typing" ? "mascot-typing" : "mascot-sweep"} ${className ?? ""}`}
      aria-hidden="true"
      title={mode === "typing" ? "typing…" : "sweeping…"}
    >
      <svg width={size} height={size} viewBox="0 0 64 64" data-bip="buddy">
        <defs>
          <linearGradient id={`bip-buddy-${uid}`} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={m.body[0]} />
            <stop offset="100%" stopColor={m.body[1]} />
          </linearGradient>
        </defs>

        {/* Horn */}
        <path d="M32 17 L30.5 11 L33.5 11.5 Z" fill={BONE} />
        <circle cx="30.8" cy="11.2" r="1.5" fill={m.accent} className="mascot-glow" />

        {/* Head */}
        <rect x="17" y="16" width="30" height="28" rx="11" fill={`url(#bip-buddy-${uid})`} />

        {/* Brow plate */}
        <rect x="20" y="20" width="24" height="4" rx="2" fill={m.plateDark} />

        {/* Visor */}
        <rect x="21" y="24" width="22" height="11" rx="5.5" fill={VISOR} />
        <rect x="31.4" y="24" width="1.2" height="10" fill={m.accent} opacity="0.9" />

        {/* Eyes — small inline versions; not GSAP-morphed because the
            buddy is a passive icon, not a performative character. */}
        {sweep ? (
          <>
            <path d="M25 30.5 Q26.5 28.8 28 30.5" stroke={m.accent} strokeWidth="1.6" strokeLinecap="round" fill="none" />
            <path d="M36 30.5 Q37.5 28.8 39 30.5" stroke={m.accent} strokeWidth="1.6" strokeLinecap="round" fill="none" />
          </>
        ) : (
          <>
            <circle cx="26.8" cy="29.5" r="1.9" fill={m.accent} />
            <circle cx="37.2" cy="29.5" r="1.9" fill={m.accent} />
            <circle cx="26.2" cy="29" r="0.6" fill={VISOR} />
            <circle cx="36.6" cy="29" r="0.6" fill={VISOR} />
          </>
        )}

        {/* Mouth — neutral smile for both modes */}
        <path
          d={mode === "sweeping" ? MOUTH_O : MOUTH_GRIN}
          stroke={m.accent}
          strokeWidth="1.6"
          strokeLinecap="round"
          fill="none"
        />

        {/* Chest core */}
        <circle cx="32" cy="43" r="1.8" fill={m.accent} className="mascot-glow" />

        {sweep && (
          <>
            <g className="broom-wiggle">
              <line x1="38" y1="52" x2="48" y2="41" stroke="#d7d7de" strokeWidth="2" strokeLinecap="round" />
              <path d="M46 39 L52 33 L56 37 L50 43 Z" fill="#ff6b5e" />
            </g>
            <circle className="sweep-dust" cx="26" cy="44" r="1.2" fill="#ffc94d" style={{ "--dx": "-5px", "--dy": "-4px" } as CSSProperties} />
            <circle className="sweep-dust" cx="22" cy="38" r="1" fill="#5bc9ff" style={{ "--dx": "-6px", "--dy": "-3px", animationDelay: "0.25s" } as CSSProperties} />
            <circle className="sweep-dust" cx="29" cy="50" r="1" fill="#b28dff" style={{ "--dx": "-7px", "--dy": "2px", animationDelay: "0.12s" } as CSSProperties} />
          </>
        )}
      </svg>
    </span>
  );
}
