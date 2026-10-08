export type OrbState = "idle" | "thinking" | "speaking";

interface StateStyle {
  /** Brightness (0..1) of the shadow, mid and highlight tones of the metal. */
  tones: [number, number, number];
  baseSpeed: number;
  glow: number;
  /** How strongly liquid ripples travel across the surface (0 = barely). */
  ripple: number;
}

// A monochrome chrome sphere: state shows up as brightness and pace, never
// as hue.
const STATE_STYLE: Record<OrbState, StateStyle> = {
  idle: { tones: [0.2, 0.5, 0.86], baseSpeed: 0.28, glow: 0.3, ripple: 0.1 },
  // Generating a reply: the strongest, flowing ripples.
  thinking: { tones: [0.12, 0.36, 0.66], baseSpeed: 1.5, glow: 0.5, ripple: 1.0 },
  // Writing the reply out.
  speaking: { tones: [0.34, 0.72, 1.0], baseSpeed: 1.1, glow: 0.7, ripple: 0.7 },
};

function lerp(from: number, to: number, t: number): number {
  return from + (to - from) * t;
}

const VERTEX_SRC = `
attribute vec2 aPos;
void main() { gl_Position = vec4(aPos, 0.0, 1.0); }
`;

// A raymarched, noise-displaced glass sphere. The surface is translucent
// (alpha rises toward the silhouette via a fresnel term) so whatever is
// behind the window shows through, and a soft halo fades out before the
// canvas edge. Output is premultiplied alpha.
const FRAGMENT_SRC = `
precision highp float;
uniform vec2 uRes;
uniform float uPhase;
uniform float uTime;
uniform float uWave;
uniform float uRipple;
uniform float uLevel;
uniform float uGlow;
uniform vec3 uC0;
uniform vec3 uC1;
uniform vec3 uC2;

float hash(vec3 p) {
  p = fract(p * 0.3183099 + 0.1);
  p *= 17.0;
  return fract(p.x * p.y * p.z * (p.x + p.y + p.z));
}

float noise(vec3 x) {
  vec3 i = floor(x);
  vec3 f = fract(x);
  f = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
  return mix(
    mix(mix(hash(i + vec3(0.0, 0.0, 0.0)), hash(i + vec3(1.0, 0.0, 0.0)), f.x),
        mix(hash(i + vec3(0.0, 1.0, 0.0)), hash(i + vec3(1.0, 1.0, 0.0)), f.x), f.y),
    mix(mix(hash(i + vec3(0.0, 0.0, 1.0)), hash(i + vec3(1.0, 0.0, 1.0)), f.x),
        mix(hash(i + vec3(0.0, 1.0, 1.0)), hash(i + vec3(1.0, 1.0, 1.0)), f.x), f.y),
    f.z);
}

mat2 rot(float a) {
  float c = cos(a);
  float s = sin(a);
  return mat2(c, -s, s, c);
}

float field(vec3 p) {
  vec3 q = p;
  q.xz = rot(uPhase * 0.35) * q.xz;
  q.yz = rot(uPhase * 0.21) * q.yz;
  float n = noise(q * 1.35 + vec3(0.0, uPhase * 0.7, uPhase * 0.4));
  n += 0.28 * noise(q * 2.6 - vec3(uPhase * 0.5, 0.0, uPhase * 0.3));
  return n;
}

float map(vec3 p) {
  float r = length(p);
  vec3 dir = p / max(r, 1e-4);
  // Two slowly wandering axes; waves travel along each and interfere, which
  // reads as liquid rippling across the surface rather than a pulse.
  vec3 axisA = normalize(vec3(sin(uTime * 0.21), cos(uTime * 0.17), 0.7));
  vec3 axisB = normalize(vec3(cos(uTime * 0.13), 0.5, sin(uTime * 0.19)));
  float ripple = sin(dot(dir, axisA) * 4.5 - uWave) + 0.6 * sin(dot(dir, axisB) * 3.4 + uWave * 0.75);
  float amp = 0.06 + 0.17 * uLevel;
  float breathe = 0.014 * sin(uTime * 0.9);
  return r - 0.55 - breathe - amp * (field(p) - 0.64) - 0.032 * uRipple * ripple;
}

vec3 normalAt(vec3 p) {
  vec2 e = vec2(0.004, -0.004);
  return normalize(
    e.xyy * map(p + e.xyy) + e.yyx * map(p + e.yyx) +
    e.yxy * map(p + e.yxy) + e.xxx * map(p + e.xxx));
}

void main() {
  vec2 uv = (gl_FragCoord.xy * 2.0 - uRes) / min(uRes.x, uRes.y);
  vec3 ro = vec3(0.0, 0.0, -2.4);
  vec3 rd = normalize(vec3(uv, 1.9));

  float t = 0.0;
  float minD = 1e3;
  vec3 pMin = ro;
  for (int i = 0; i < 56; i++) {
    vec3 p = ro + rd * t;
    float d = map(p);
    if (d < minD) { minD = d; pMin = p; }
    if (d < 0.002) break;
    t += d * 0.8;
    if (t > 6.0) break;
  }

  float edgeFade = smoothstep(1.0, 0.5, length(uv));
  float glowAmt = exp(-max(minD, 0.0) * 5.0) * (0.3 + uGlow * 0.9) * (0.55 + uLevel * 0.9) * edgeFade;
  vec3 haloCol = mix(uC0, uC2, 0.5 + 0.5 * sin(uv.x * 2.2 + uv.y * 1.3 + uPhase));
  vec4 halo = vec4(haloCol * glowAmt, glowAmt * 0.55);

  float cover = smoothstep(0.02, 0.0, minD);
  if (cover <= 0.0) {
    gl_FragColor = halo;
    return;
  }

  vec3 n = normalAt(pMin);
  float fres = pow(1.0 - max(dot(n, -rd), 0.0), 2.4);
  vec3 L = normalize(vec3(-0.5, 0.7, -0.6));
  float diff = max(dot(n, L), 0.0);
  float spec = pow(max(dot(reflect(rd, n), L), 0.0), 48.0);

  float k = 0.5 + 0.5 * sin(pMin.y * 2.6 + pMin.x * 1.6 + uPhase * 1.1 + field(pMin) * 2.0);
  vec3 col = mix(uC0, uC1, k);
  col = mix(col, uC2, clamp(fres * 0.9 + (1.0 - k) * 0.15, 0.0, 1.0));
  vec3 refl = reflect(rd, n);
  float band = 0.5 + 0.5 * sin(refl.y * 7.0 + refl.x * 2.0 + uPhase * 0.4);
  col = col * (0.45 + 0.75 * diff) * (0.7 + 0.6 * band) + uC2 * fres * 0.5 + vec3(spec) * 1.1;
  col += uC0 * (1.0 - fres) * (0.12 + 0.3 * uLevel);

  float alpha = clamp(0.5 + fres * 0.5 + spec, 0.0, 1.0);
  vec4 body = vec4(col * alpha, alpha);
  gl_FragColor = mix(halo, body, cover);
}
`;

export class SiriOrb {
  private canvas: HTMLCanvasElement;
  private gl: WebGLRenderingContext | null;
  private program: WebGLProgram | null = null;
  private uniforms: Record<string, WebGLUniformLocation | null> = {};
  private raf = 0;
  private phase = 0;
  private time = 0;
  private wave = 0;
  private lastFrame = 0;
  private hover = false;
  private state: OrbState = "idle";
  private level = 0;
  private smoothLevel = 0;
  private style: StateStyle;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    this.style = { ...STATE_STYLE.idle, tones: [...STATE_STYLE.idle.tones] };

    this.gl = canvas.getContext("webgl", {
      alpha: true,
      premultipliedAlpha: true,
      antialias: true,
    });
    if (!this.gl) {
      console.warn("WebGL unavailable — orb will not render.");
      return;
    }
    this.initGl(this.gl);
  }

  private initGl(gl: WebGLRenderingContext) {
    const compile = (type: number, src: string) => {
      const shader = gl.createShader(type)!;
      gl.shaderSource(shader, src);
      gl.compileShader(shader);
      if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
        throw new Error(gl.getShaderInfoLog(shader) ?? "shader compile failed");
      }
      return shader;
    };

    const program = gl.createProgram()!;
    gl.attachShader(program, compile(gl.VERTEX_SHADER, VERTEX_SRC));
    gl.attachShader(program, compile(gl.FRAGMENT_SHADER, FRAGMENT_SRC));
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      throw new Error(gl.getProgramInfoLog(program) ?? "program link failed");
    }
    gl.useProgram(program);

    const buffer = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    const loc = gl.getAttribLocation(program, "aPos");
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);

    for (const name of ["uRes", "uPhase", "uTime", "uWave", "uRipple", "uLevel", "uGlow", "uC0", "uC1", "uC2"]) {
      this.uniforms[name] = gl.getUniformLocation(program, name);
    }
    gl.clearColor(0, 0, 0, 0);
    this.program = program;
  }

  setState(state: OrbState) {
    this.state = state;
  }

  /** While the pointer is over the orb it ripples at full strength. */
  setHover(hover: boolean) {
    this.hover = hover;
  }

  setLevel(level: number) {
    this.level = Math.min(1, Math.max(0, level));
  }

  start() {
    if (!this.gl || !this.program) return;
    const loop = (now: number) => {
      const dt = this.lastFrame ? Math.min(0.05, (now - this.lastFrame) / 1000) : 1 / 60;
      this.lastFrame = now;
      this.draw(dt);
      this.raf = requestAnimationFrame(loop);
    };
    this.raf = requestAnimationFrame(loop);
  }

  stop() {
    cancelAnimationFrame(this.raf);
  }

  // Keep the backing store matched to the on-screen size so the orb stays
  // crisp while the layout animates between its large and small forms.
  private syncSize() {
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    const w = Math.max(1, Math.round(this.canvas.clientWidth * dpr));
    const h = Math.max(1, Math.round(this.canvas.clientHeight * dpr));
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
  }

  private draw(dt: number) {
    const gl = this.gl!;
    const base = STATE_STYLE[this.state];
    // Hovering lifts the ripple, pace and glow to at least a lively level;
    // the usual per-second easing below makes it swell in and fade out.
    const target: StateStyle = this.hover
      ? {
          ...base,
          ripple: Math.max(base.ripple, 1.0),
          baseSpeed: Math.max(base.baseSpeed, 1.3),
          glow: Math.max(base.glow, 0.65),
        }
      : base;

    // Ease every visual property toward its target. The rates are per second,
    // not per frame, so the motion is equally smooth at any frame rate.
    const ease = 1 - Math.exp(-dt * 2.5);
    this.style.tones = [
      lerp(this.style.tones[0], target.tones[0], ease),
      lerp(this.style.tones[1], target.tones[1], ease),
      lerp(this.style.tones[2], target.tones[2], ease),
    ];
    this.style.baseSpeed = lerp(this.style.baseSpeed, target.baseSpeed, ease);
    this.style.glow = lerp(this.style.glow, target.glow, ease);
    this.style.ripple = lerp(this.style.ripple, target.ripple, ease);
    this.smoothLevel += (this.level - this.smoothLevel) * (1 - Math.exp(-dt * 8));

    // Even with no audio level (e.g. while a reply is being generated) the
    // surface keeps a slow swell that scales with the ripple strength.
    this.time += dt;
    const swell = 0.55 + 0.45 * Math.sin(this.time * 1.6);
    const drive = this.smoothLevel * 0.8 + this.style.ripple * 0.22 * swell;

    // Integrate speed into phases so changing state changes the pace
    // smoothly instead of jumping the noise field.
    this.phase += dt * this.style.baseSpeed * (0.7 + drive * 1.4);
    this.wave += dt * (0.5 + 2.4 * this.style.ripple);

    this.syncSize();
    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.clear(gl.COLOR_BUFFER_BIT);

    const s = this.style;
    const u = this.uniforms;
    gl.uniform2f(u.uRes, this.canvas.width, this.canvas.height);
    gl.uniform1f(u.uPhase, this.phase);
    gl.uniform1f(u.uTime, this.time);
    gl.uniform1f(u.uWave, this.wave);
    gl.uniform1f(u.uRipple, Math.min(1.3, s.ripple + this.smoothLevel * 0.4));
    gl.uniform1f(u.uLevel, drive);
    gl.uniform1f(u.uGlow, s.glow);
    const [t0, t1, t2] = s.tones;
    gl.uniform3f(u.uC0, t0, t0, t0);
    gl.uniform3f(u.uC1, t1, t1, t1);
    gl.uniform3f(u.uC2, t2, t2, t2);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }
}
