// The update animation, drawn on a full-window canvas:
//  - playWarp: the update. A long flight at rocket speed through endless
//    space: the stars stretch into streaks, the ship (the orb) shudders at
//    full speed, then everything slows for arrival.
// Black, white and grey only, like the rest of the app.

export interface SpaceRun {
  /** Fades the animation out and then calls `done`. */
  stop(done?: () => void): void;
}

const reducedMotion = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;

const clamp = (v: number, lo = 0, hi = 1) => Math.min(hi, Math.max(lo, v));
const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
const easeInOut = (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);

/** Sizes the canvas to its CSS size at the screen's pixel density. */
function fit(canvas: HTMLCanvasElement): { ctx: CanvasRenderingContext2D; w: number; h: number } | null {
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return { ctx, w, h };
}

// ---------- update: flight at rocket speed through endless space ----------

interface Star {
  x: number;
  y: number;
  z: number;
  var: number;
  light: number;
}

export interface WarpOptions {
  /** Called every frame with 0..1 of the flight. */
  onProgress?: (progress: number) => void;
}

export function playWarp(canvas: HTMLCanvasElement, durationMs: number, options: WarpOptions = {}): SpaceRun {
  const duration = durationMs / 1000;
  const still = reducedMotion();
  const newStar = (anywhere: boolean): Star => ({
    // Spread around the centre, with a small gap so none sit dead ahead.
    x: (Math.random() < 0.5 ? -1 : 1) * (0.04 + Math.random() * 1.6),
    y: (Math.random() < 0.5 ? -1 : 1) * (0.04 + Math.random() * 1.6),
    z: anywhere ? 0.05 + Math.random() * 0.95 : 1,
    var: 0.55 + Math.random() * 0.9,
    light: 0.35 + Math.random() * 0.65,
  });
  const stars = Array.from({ length: 460 }, () => newStar(true));

  let frame = 0;
  let finished = false;
  let fadingOut: { from: number; done?: () => void } | null = null;
  const startedAt = performance.now();
  let last = startedAt;

  /** How fast the ship is going: pull away, hold full speed, slow for arrival. */
  const speedAt = (t: number): number => {
    const up = 3.2;
    const down = 3.2;
    if (t < up) return lerp(0.18, 2.7, easeInOut(t / up));
    if (t < duration - down) return 2.7 + 0.25 * Math.sin(t * 1.9);
    if (t < duration) return lerp(2.7, 0.25, easeInOut((t - (duration - down)) / down));
    return 0.3;
  };

  const finish = () => {
    if (finished) return;
    finished = true;
    cancelAnimationFrame(frame);
    const f = fit(canvas);
    f?.ctx.clearRect(0, 0, f.w, f.h);
    fadingOut?.done?.();
  };

  const step = (now: number) => {
    const f = fit(canvas);
    if (!f) return finish();
    const { ctx, w, h } = f;
    const t = (now - startedAt) / 1000;
    const dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    const v = still ? 0 : speedAt(t);
    options.onProgress?.(clamp(t / duration));

    let alpha = clamp(t / 0.7);
    if (fadingOut) {
      alpha *= 1 - clamp((now - fadingOut.from) / 500);
      if (alpha <= 0) return finish();
    }

    const cx = w / 2;
    const cy = h / 2;
    const focal = Math.min(w, h) * 0.34;
    const intensity = clamp(v / 2.7);

    ctx.clearRect(0, 0, w, h);
    // Deep space, a little lighter where the ship is heading.
    const space = ctx.createRadialGradient(cx, cy, 0, cx, cy, Math.hypot(w, h) * 0.6);
    space.addColorStop(0, `rgba(30,32,38,${0.97 * alpha})`);
    space.addColorStop(0.5, `rgba(8,8,11,${0.98 * alpha})`);
    space.addColorStop(1, `rgba(1,1,2,${alpha})`);
    ctx.fillStyle = space;
    ctx.fillRect(0, 0, w, h);

    ctx.lineCap = "round";
    for (const s of stars) {
      const prevZ = s.z + v * 0.05 * s.var + 0.002;
      s.z -= v * dt * 0.42 * s.var;
      if (s.z <= 0.03) Object.assign(s, newStar(false));
      const sx = cx + (s.x / s.z) * focal;
      const sy = cy + (s.y / s.z) * focal;
      if (sx < -80 || sx > w + 80 || sy < -80 || sy > h + 80) {
        Object.assign(s, newStar(false));
        continue;
      }
      const px = cx + (s.x / prevZ) * focal;
      const py = cy + (s.y / prevZ) * focal;
      const near = 1 - s.z;
      const a = clamp(near * 1.4, 0.04, 1) * s.light * alpha;
      ctx.strokeStyle = `rgba(255,255,255,${a.toFixed(3)})`;
      ctx.lineWidth = 0.5 + near * (1.2 + intensity * 1.6);
      ctx.beginPath();
      ctx.moveTo(px, py);
      ctx.lineTo(sx, sy);
      ctx.stroke();
    }

    // At speed the edges of the view darken, like a tunnel.
    if (intensity > 0.05) {
      const tunnel = ctx.createRadialGradient(cx, cy, Math.min(w, h) * 0.25, cx, cy, Math.hypot(w, h) * 0.55);
      tunnel.addColorStop(0, "rgba(0,0,0,0)");
      tunnel.addColorStop(1, `rgba(0,0,0,${(0.55 * intensity * alpha).toFixed(3)})`);
      ctx.fillStyle = tunnel;
      ctx.fillRect(0, 0, w, h);
    }

    if (still) return; // reduced motion: one calm frame, no movement
    frame = requestAnimationFrame(step);
  };
  frame = requestAnimationFrame(step);

  return {
    stop(done) {
      if (finished) return done?.();
      fadingOut = { from: performance.now(), done };
      if (still) finish();
    },
  };
}
