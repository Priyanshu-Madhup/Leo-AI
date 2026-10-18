// Interactive charts for the chat.
//
// A fenced `chart` block holds a JSON spec written by the model (the format is
// described in widget.rs). The chart is drawn as plain SVG, no library, and
// every piece of text is set with textContent, so a spec can never inject
// markup. Hover or touch shows a crosshair and tooltip; the legend chips hide
// and show series.
//
// Spec: { type: "line" | "area" | "bar", title?, subtitle?, unit?, footer?,
//         labels: string[], series: [{ name, values: number[] }] }

const NS = "http://www.w3.org/2000/svg";
const W = 320;
const H = 170;
const M = { l: 38, r: 10, t: 10, b: 24 };
const PALETTE = ["#8ab4f8", "#81c995", "#fdd663", "#f28b82", "#c58af9"];
const MAX_POINTS = 60;
const MAX_SERIES = 5;

type Kind = "line" | "area" | "bar";
interface Series {
  name: string;
  color: string;
  values: (number | null)[];
}

const isObj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const text = (v: unknown, max = 80): string => (typeof v === "string" || typeof v === "number" ? String(v).slice(0, max) : "");

function s(tag: string, attrs: Record<string, string | number> = {}): SVGElement {
  const el = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, String(v));
  return el;
}

function div(cls: string, content = ""): HTMLDivElement {
  const el = document.createElement("div");
  el.className = cls;
  if (content) el.textContent = content;
  return el;
}

/** A round step (1, 2, 5 x 10^n) near `rough`, for axis ticks. */
function niceStep(rough: number): number {
  const pow = Math.pow(10, Math.floor(Math.log10(rough)));
  const f = rough / pow;
  return (f <= 1 ? 1 : f <= 2 ? 2 : f <= 5 ? 5 : 10) * pow;
}

const nf = new Intl.NumberFormat(undefined, { maximumFractionDigits: 2 });

export function chartCard(json: string): HTMLElement | null {
  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch {
    return null; // half-written or invalid: draw nothing
  }
  if (!isObj(raw) || !Array.isArray(raw.labels) || !Array.isArray(raw.series)) return null;

  const kind: Kind = raw.type === "bar" || raw.type === "area" ? raw.type : "line";
  const labels = raw.labels.slice(0, MAX_POINTS).map((l) => text(l, 24));
  const unit = text(raw.unit, 8);
  const series: Series[] = [];
  for (const item of raw.series.slice(0, MAX_SERIES)) {
    if (!isObj(item) || !Array.isArray(item.values)) continue;
    const given: unknown[] = item.values;
    series.push({
      name: text(item.name, 30),
      color: PALETTE[series.length % PALETTE.length],
      values: labels.map((_, i) => {
        const v = given[i];
        return typeof v === "number" && Number.isFinite(v) ? v : null;
      }),
    });
  }
  if (!labels.length || !series.length || !series.some((x) => x.values.some((v) => v !== null))) return null;

  const fmt = (v: number) => `${nf.format(v)}${unit ? ` ${unit}` : ""}`;
  const hidden = new Set<number>();
  let active = -1;

  const card = div("chart");
  const title = text(raw.title, 80);
  const subtitle = text(raw.subtitle, 100);
  if (title) card.append(div("chart-title", title));
  if (subtitle) card.append(div("chart-sub", subtitle));

  const frame = div("chart-frame");
  const svg = s("svg", { viewBox: `0 0 ${W} ${H}`, class: "chart-svg", role: "img", "aria-label": title || "Chart" }) as SVGSVGElement;
  const tip = div("chart-tip");
  frame.append(svg, tip);
  card.append(frame);

  const legend = div("chart-legend");
  card.append(legend);

  const px = (i: number): number => {
    const w = W - M.l - M.r;
    if (kind === "bar") return M.l + ((i + 0.5) / labels.length) * w;
    return labels.length === 1 ? M.l + w / 2 : M.l + (i / (labels.length - 1)) * w;
  };

  const draw = () => {
    svg.replaceChildren();
    const shown = series.filter((_, i) => !hidden.has(i));
    const all = shown.flatMap((x) => x.values).filter((v): v is number => v !== null);
    let lo = all.length ? Math.min(...all) : 0;
    let hi = all.length ? Math.max(...all) : 1;
    if (kind !== "line") lo = Math.min(0, lo);
    if (hi === lo) hi = lo + 1;
    const step = niceStep((hi - lo) / 4);
    lo = Math.floor(lo / step) * step;
    hi = Math.ceil(hi / step) * step;
    const py = (v: number) => M.t + (1 - (v - lo) / (hi - lo)) * (H - M.t - M.b);

    for (let v = lo; v <= hi + step / 2; v += step) {
      const y = py(v);
      svg.append(s("line", { x1: M.l, x2: W - M.r, y1: y, y2: y, class: "chart-grid" }));
      const t = s("text", { x: M.l - 6, y: y + 3, class: "chart-axis", "text-anchor": "end" });
      t.textContent = nf.format(Number(v.toFixed(6)));
      svg.append(t);
    }
    const every = Math.ceil(labels.length / 6);
    labels.forEach((l, i) => {
      if (i % every) return;
      const t = s("text", { x: px(i), y: H - 7, class: "chart-axis", "text-anchor": "middle" });
      t.textContent = l;
      svg.append(t);
    });

    const base = py(Math.max(lo, Math.min(0, hi)));
    if (kind === "bar") {
      const band = (W - M.l - M.r) / labels.length;
      const bw = Math.max(2, Math.min(28, (band * 0.7) / Math.max(1, shown.length)));
      shown.forEach((x, k) => {
        x.values.forEach((v, i) => {
          if (v === null) return;
          const y = py(v);
          const left = px(i) - (bw * shown.length) / 2 + k * bw;
          svg.append(
            s("rect", { x: left + 0.5, y: Math.min(y, base), width: Math.max(1, bw - 1), height: Math.abs(base - y), rx: 2, fill: x.color, class: "chart-bar", "data-i": i }),
          );
        });
      });
    } else {
      for (const x of shown) {
        const segments: string[][] = [[]];
        x.values.forEach((v, i) => {
          if (v === null) segments.push([]);
          else segments[segments.length - 1].push(`${px(i).toFixed(1)},${py(v).toFixed(1)}`);
        });
        for (const seg of segments.filter((g) => g.length)) {
          if (kind === "area") {
            const first = seg[0].split(",")[0];
            const last = seg[seg.length - 1].split(",")[0];
            svg.append(s("polygon", { points: `${first},${base} ${seg.join(" ")} ${last},${base}`, fill: x.color, "fill-opacity": 0.16 }));
          }
          svg.append(s("polyline", { points: seg.join(" "), fill: "none", stroke: x.color, "stroke-width": 2, "stroke-linejoin": "round", "stroke-linecap": "round" }));
          if (seg.length === 1) {
            const [cx, cy] = seg[0].split(",");
            svg.append(s("circle", { cx, cy, r: 3, fill: x.color }));
          }
        }
      }
    }

    // Hover layer: crosshair, dots and a capture rectangle over the plot.
    const cross = s("line", { y1: M.t, y2: H - M.b, class: "chart-cross", visibility: "hidden" });
    svg.append(cross);
    const dots = shown.map((x) => {
      const d = s("circle", { r: 4, fill: x.color, class: "chart-dot", visibility: "hidden" });
      svg.append(d);
      return d;
    });
    const capture = s("rect", { x: M.l, y: M.t, width: W - M.l - M.r, height: H - M.t - M.b, fill: "transparent" });
    svg.append(capture);

    const hide = () => {
      active = -1;
      cross.setAttribute("visibility", "hidden");
      dots.forEach((d) => d.setAttribute("visibility", "hidden"));
      tip.classList.remove("on");
    };
    const show = (clientX: number) => {
      const rect = svg.getBoundingClientRect();
      if (!rect.width) return;
      const x = ((clientX - rect.left) / rect.width) * W;
      let best = 0;
      for (let i = 1; i < labels.length; i++) if (Math.abs(px(i) - x) < Math.abs(px(best) - x)) best = i;
      active = best;
      cross.setAttribute("x1", String(px(best)));
      cross.setAttribute("x2", String(px(best)));
      cross.setAttribute("visibility", kind === "bar" ? "hidden" : "visible");
      shown.forEach((sr, k) => {
        const v = sr.values[best];
        dots[k].setAttribute("visibility", v === null || kind === "bar" ? "hidden" : "visible");
        if (v !== null) {
          dots[k].setAttribute("cx", String(px(best)));
          dots[k].setAttribute("cy", String(py(v)));
        }
      });
      svg.querySelectorAll<SVGElement>(".chart-bar").forEach((b) => b.classList.toggle("dim", b.getAttribute("data-i") !== String(best)));

      tip.replaceChildren(div("chart-tip-label", labels[best]));
      for (const sr of shown) {
        const v = sr.values[best];
        if (v === null) continue;
        const row = div("chart-tip-row");
        const swatch = document.createElement("span");
        swatch.className = "chart-swatch";
        swatch.style.background = sr.color;
        row.append(swatch, document.createTextNode(`${sr.name ? `${sr.name}: ` : ""}${fmt(v)}`));
        tip.append(row);
      }
      const frac = px(best) / W;
      tip.style.left = `${frac * 100}%`;
      tip.classList.toggle("flip", frac > 0.6);
      tip.classList.add("on");
    };

    capture.addEventListener("pointermove", (e) => show((e as PointerEvent).clientX));
    capture.addEventListener("pointerdown", (e) => show((e as PointerEvent).clientX));
    capture.addEventListener("pointerleave", hide);
    if (active >= 0) hide();
  };

  // Legend chips double as toggles; a single series needs no legend.
  if (series.length > 1) {
    series.forEach((sr, i) => {
      const chip = document.createElement("button");
      chip.type = "button";
      chip.className = "chart-chip";
      chip.setAttribute("aria-pressed", "true");
      const swatch = document.createElement("span");
      swatch.className = "chart-swatch";
      swatch.style.background = sr.color;
      chip.append(swatch, document.createTextNode(sr.name || `Series ${i + 1}`));
      chip.addEventListener("click", () => {
        if (hidden.has(i)) hidden.delete(i);
        else if (hidden.size < series.length - 1) hidden.add(i);
        chip.classList.toggle("off", hidden.has(i));
        chip.setAttribute("aria-pressed", String(!hidden.has(i)));
        draw();
      });
      legend.append(chip);
    });
  }

  const footer = text(raw.footer, 120);
  if (footer) card.append(div("chart-foot", footer));
  draw();
  return card;
}
