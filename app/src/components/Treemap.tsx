import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { hierarchy, treemap, treemapSquarify, type HierarchyRectangularNode } from "d3-hierarchy";
import type { TreemapNode } from "../api";
import { bytes, percent, plural } from "../format";

type HNode = HierarchyRectangularNode<TreemapNode>;

/** Brand data-viz order for treemap folders (see brand/BRAND.md). */
export const VIZ = [
  "powder",
  "sage",
  "lilac",
  "peach",
  "mist",
  "blush",
  "pistachio",
  "sand",
  "mauve",
  "cocoa",
  "slate",
  "stone",
] as const;

/** CSS color for viz slot `i`: the pastel fill, or its deep tone for text and bars. */
export function viz(i: number, deep = false): string {
  return `var(--viz-${VIZ[i % VIZ.length]}${deep ? "-deep" : ""})`;
}

const HEADER = 20;

function shade(hex: string, amt: number): string {
  // amt > 0 lightens towards white, < 0 darkens towards black.
  const n = parseInt(hex.slice(1), 16);
  let r = (n >> 16) & 255;
  let g = (n >> 8) & 255;
  let b = n & 255;
  const t = amt < 0 ? 0 : 255;
  const p = Math.abs(amt);
  r = Math.round((t - r) * p + r);
  g = Math.round((t - g) * p + g);
  b = Math.round((t - b) * p + b);
  return `rgb(${r},${g},${b})`;
}

function alpha(hex: string, a: number): string {
  const n = parseInt(hex.slice(1), 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${a})`;
}

function topIndex(n: HNode): number {
  let cur: HNode = n;
  while (cur.depth > 1 && cur.parent) cur = cur.parent;
  return cur.parent ? cur.parent.children!.indexOf(cur) : 0;
}

function ellipsize(ctx: CanvasRenderingContext2D, text: string, max: number): string {
  if (ctx.measureText(text).width <= max) return text;
  let lo = 0;
  let hi = text.length;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (ctx.measureText(text.slice(0, mid) + "…").width <= max) lo = mid;
    else hi = mid - 1;
  }
  return lo > 0 ? text.slice(0, lo) + "…" : "";
}

let hatch: CanvasPattern | null = null;
let hatchTheme = "";
function hatchPattern(ctx: CanvasRenderingContext2D, stroke: string, bg: string): CanvasPattern | null {
  const key = stroke + bg;
  if (hatch && hatchTheme === key) return hatch;
  const c = document.createElement("canvas");
  c.width = c.height = 10;
  const x = c.getContext("2d")!;
  x.fillStyle = bg;
  x.fillRect(0, 0, 10, 10);
  x.strokeStyle = stroke;
  x.lineWidth = 2;
  x.beginPath();
  x.moveTo(-2, 12);
  x.lineTo(12, -2);
  x.moveTo(-2, 2);
  x.lineTo(2, -2);
  x.moveTo(8, 12);
  x.lineTo(12, 8);
  x.stroke();
  hatch = ctx.createPattern(c, "repeat");
  hatchTheme = key;
  return hatch;
}

export function Treemap({
  data,
  selected,
  onOpen,
  onSelect,
  onContext,
}: {
  data: TreemapNode | null;
  selected: Set<number>;
  onOpen: (id: number) => void;
  onSelect: (id: number, additive: boolean) => void;
  onContext: (id: number, x: number, y: number) => void;
}) {
  const wrap = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [hover, setHover] = useState<{ node: HNode; x: number; y: number } | null>(null);
  const [themeTick, setThemeTick] = useState(0);

  useEffect(() => {
    const el = wrap.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    const onTheme = () => setThemeTick((t) => t + 1);
    window.addEventListener("roomy-theme", onTheme);
    return () => {
      ro.disconnect();
      window.removeEventListener("roomy-theme", onTheme);
    };
  }, []);

  const root = useMemo(() => {
    if (!data || size.w < 10 || size.h < 10) return null;
    const h = hierarchy<TreemapNode>(data, (d) => (d.children.length ? d.children : null))
      .sum((d) => (d.children.length ? 0 : Math.max(d.size, 0)))
      .sort((a, b) => (b.value ?? 0) - (a.value ?? 0));
    return treemap<TreemapNode>()
      .size([size.w, size.h])
      .tile(treemapSquarify.ratio(1.25))
      .paddingInner(2)
      .paddingOuter((n) => (n.depth === 0 ? 0 : 3))
      .paddingTop((n) => {
        if (n.depth === 0) return 0;
        const w = n.x1 - n.x0;
        const hh = n.y1 - n.y0;
        return w > 70 && hh > 60 ? HEADER : 3;
      })
      .round(true)(h);
  }, [data, size]);

  const nodes = useMemo(() => (root ? root.descendants().filter((n) => n.depth > 0) : []), [root]);

  const draw = useCallback(() => {
    const c = canvas.current;
    if (!c || !root) return;
    const dpr = window.devicePixelRatio || 1;
    c.width = Math.round(size.w * dpr);
    c.height = Math.round(size.h * dpr);
    const ctx = c.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const css = getComputedStyle(document.documentElement);
    const v = (name: string, fallback: string) => css.getPropertyValue(name).trim() || fallback;
    const surface = v("--surface", "#fbfaf6");
    const text = v("--ink", "#1b1a17");
    const faint = v("--ink-3", "#6a655b");
    const sunken = v("--sunken", "#eae7df");
    const label = v("--viz-label", "#1b1a17");
    const highlight = v("--viz-highlight", "#f6d374");
    const stone = v("--viz-stone", "#d8d3ca");
    const fills = VIZ.map((name) => v(`--viz-${name}`, "#d8d3ca"));
    const light = document.documentElement.dataset.theme !== "dark";
    ctx.fillStyle = surface;
    ctx.fillRect(0, 0, size.w, size.h);
    const font = getComputedStyle(document.body).fontFamily;

    for (const n of nodes) {
      const w = n.x1 - n.x0;
      const h = n.y1 - n.y0;
      if (w < 1 || h < 1) continue;
      const d = n.data;
      const base = fills[topIndex(n) % fills.length];
      const r = Math.min(8, w / 3, h / 3);
      const isFrame = !!n.children;
      const sel = d.id !== null && selected.has(d.id);
      const hidden = d.kind === "gap" || d.kind === "gapPart";
      ctx.beginPath();
      ctx.roundRect(n.x0, n.y0, w, h, r);
      if (sel) {
        // The one butter thing in the view: what you picked.
        ctx.fillStyle = highlight;
      } else if (hidden) {
        ctx.fillStyle = hatchPattern(ctx, alpha(light ? "#5C584F" : "#1B1A17", 0.3), stone) ?? stone;
      } else if (d.kind === "rest") {
        ctx.fillStyle = sunken;
      } else if (isFrame) {
        ctx.fillStyle = alpha(base, light ? 0.42 : 0.2);
      } else {
        const depthShade = -(n.depth - 1) * 0.045;
        ctx.fillStyle = shade(base, d.kind === "dir" ? depthShade - 0.05 : depthShade);
      }
      ctx.fill();

      if (hover?.node === n && !sel) {
        ctx.lineWidth = 2;
        ctx.strokeStyle = text;
        ctx.beginPath();
        ctx.roundRect(n.x0 + 1, n.y0 + 1, w - 2, h - 2, r);
        ctx.stroke();
      }

      // Labels: ink on pastel fills; theme ink on frames.
      const onFill = sel || (!isFrame && d.kind !== "rest");
      ctx.fillStyle = onFill ? label : d.kind === "rest" ? faint : text;
      ctx.textBaseline = "middle";
      if (isFrame) {
        if (w > 70 && h > 60) {
          ctx.font = `600 11.5px ${font}`;
          if (sel || hidden) ctx.fillStyle = label;
        const sizeText = bytes(d.size);
          const sw = ctx.measureText(sizeText).width;
          const title = ellipsize(ctx, d.name, w - sw - 22);
          ctx.fillText(title, n.x0 + 8, n.y0 + HEADER / 2 + 1);
          ctx.globalAlpha = 0.7;
          ctx.fillText(sizeText, n.x1 - sw - 8, n.y0 + HEADER / 2 + 1);
          ctx.globalAlpha = 1;
        }
      } else if (w > 46 && h > 26) {
        ctx.font = `600 12px ${font}`;
        const title = ellipsize(ctx, d.name, w - 14);
        const twoLines = h > 44;
        ctx.fillText(title, n.x0 + 7, n.y0 + (twoLines ? 15 : h / 2));
        if (twoLines) {
          ctx.font = `500 11px ${font}`;
          ctx.globalAlpha = 0.72;
          ctx.fillText(ellipsize(ctx, bytes(d.size), w - 14), n.x0 + 7, n.y0 + 31);
          ctx.globalAlpha = 1;
        }
      }
    }
  }, [root, nodes, size, selected, hover]);

  useEffect(() => {
    const id = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(id);
  }, [draw, themeTick]);

  const hit = (e: React.MouseEvent): HNode | null => {
    const rect = canvas.current!.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;
    for (let i = nodes.length - 1; i >= 0; i--) {
      const n = nodes[i];
      if (x >= n.x0 && x < n.x1 && y >= n.y0 && y < n.y1) return n;
    }
    return null;
  };

  const topOf = (n: HNode): HNode => {
    let cur = n;
    while (cur.depth > 1 && cur.parent) cur = cur.parent;
    return cur;
  };

  /** The deepest node that's a real item (not an aggregate). */
  const realOf = (n: HNode): HNode | null => {
    let cur: HNode | null = n;
    while (cur && cur.data.id === null) cur = cur.parent;
    return cur && cur.depth > 0 ? cur : null;
  };

  const total = data?.size ?? 0;
  const hv = hover?.node.data;

  return (
    <div className="map" ref={wrap}>
      <canvas
        ref={canvas}
        key={data?.id ?? "none"}
        style={{ width: size.w, height: size.h, animation: "fade 0.25s ease" }}
        role="img"
        aria-label={data ? `Map of ${data.name}. Use the list on the right to move with the keyboard.` : "Map"}
        onMouseMove={(e) => {
          const n = hit(e);
          setHover(n ? { node: n, x: e.clientX, y: e.clientY } : null);
        }}
        onMouseLeave={() => setHover(null)}
        onClick={(e) => {
          const n = hit(e);
          if (!n) return;
          const top = topOf(n);
          if (top.data.id === null) return;
          if (top.data.kind === "dir" && !e.ctrlKey && !e.metaKey) onOpen(top.data.id);
          else onSelect(realOf(n)?.data.id ?? top.data.id, e.ctrlKey || e.metaKey);
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          const n = hit(e);
          const r = n && realOf(n);
          if (r?.data.id != null) onContext(r.data.id, e.clientX, e.clientY);
        }}
      />
      {hover && hv && (
        <div
          className="tooltip"
          style={{
            left: Math.min(hover.x + 14, window.innerWidth - 350),
            top: Math.min(hover.y + 16, window.innerHeight - 120),
          }}
        >
          <div className="t-name">{hv.name}</div>
          <div className="t-size">{bytes(hv.size)}</div>
          <div className="muted">
            {percent(hv.size, total)} of this view
            {hv.kind === "rest" && ` · ${plural(hv.count, "item")}`}
            {hv.kind === "dir" && " · folder"}
            {(hv.kind === "gap" || hv.kind === "gapPart") && " · system space"}
          </div>
          {topOf(hover.node).data.kind === "dir" && topOf(hover.node).data.id !== null && (
            <div className="faint" style={{ marginTop: 4 }}>
              Click to open {topOf(hover.node).data.name}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
