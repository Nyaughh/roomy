import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { hierarchy, treemap, treemapSquarify, type HierarchyRectangularNode } from "d3-hierarchy";
import type { TreemapNode } from "../api";
import { bytes, percent, plural } from "../format";

type HNode = HierarchyRectangularNode<TreemapNode>;

export const PALETTE = [
  "#8B93FF",
  "#6EE7C5",
  "#7CC4FA",
  "#C4A1FF",
  "#FFB38A",
  "#F5D97A",
  "#F49AC1",
  "#5ED3D1",
  "#B5E07A",
  "#A5B4FC",
  "#FCA5A5",
  "#93C5FD",
];

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
    const surface = css.getPropertyValue("--surface").trim() || "#11131f";
    const text = css.getPropertyValue("--text").trim() || "#f2f3fa";
    const faint = css.getPropertyValue("--faint").trim() || "#6b7190";
    const raised = css.getPropertyValue("--raised-2").trim() || "#1f2336";
    const light = document.documentElement.dataset.theme === "light";
    ctx.fillStyle = surface;
    ctx.fillRect(0, 0, size.w, size.h);
    const font = getComputedStyle(document.body).fontFamily;

    for (const n of nodes) {
      const w = n.x1 - n.x0;
      const h = n.y1 - n.y0;
      if (w < 1 || h < 1) continue;
      const d = n.data;
      const base = PALETTE[topIndex(n) % PALETTE.length];
      const r = Math.min(8, w / 3, h / 3);
      const isFrame = !!n.children;
      ctx.beginPath();
      ctx.roundRect(n.x0, n.y0, w, h, r);
      if (d.kind === "gap" || d.kind === "gapPart") {
        ctx.fillStyle = hatchPattern(ctx, alpha(light ? "#8a8fa8" : "#6b7190", 0.55), raised) ?? raised;
      } else if (d.kind === "rest") {
        ctx.fillStyle = raised;
      } else if (isFrame) {
        ctx.fillStyle = alpha(base, light ? 0.22 : 0.16);
      } else {
        const depthShade = (n.depth - 1) * (light ? -0.06 : 0.07);
        ctx.fillStyle = d.kind === "dir" ? shade(base, depthShade - (light ? 0.04 : 0.1)) : shade(base, depthShade);
      }
      ctx.fill();

      const sel = d.id !== null && selected.has(d.id);
      if (sel || hover?.node === n) {
        ctx.lineWidth = 2;
        ctx.strokeStyle = sel ? text : alpha(light ? "#14162B" : "#FFFFFF", 0.85);
        ctx.beginPath();
        ctx.roundRect(n.x0 + 1, n.y0 + 1, w - 2, h - 2, r);
        ctx.stroke();
      }

      // Labels
      const onFill = !isFrame && d.kind !== "rest" && d.kind !== "gap" && d.kind !== "gapPart";
      ctx.fillStyle = onFill ? "#14162B" : d.kind === "rest" ? faint : text;
      ctx.textBaseline = "middle";
      if (isFrame) {
        if (w > 70 && h > 60) {
          ctx.font = `600 11.5px ${font}`;
          const sizeText = bytes(d.size);
          const sw = ctx.measureText(sizeText).width;
          const label = ellipsize(ctx, d.name, w - sw - 22);
          ctx.fillText(label, n.x0 + 8, n.y0 + HEADER / 2 + 1);
          ctx.globalAlpha = 0.7;
          ctx.fillText(sizeText, n.x1 - sw - 8, n.y0 + HEADER / 2 + 1);
          ctx.globalAlpha = 1;
        }
      } else if (w > 46 && h > 26) {
        ctx.font = `600 12px ${font}`;
        const label = ellipsize(ctx, d.name, w - 14);
        const twoLines = h > 44;
        ctx.fillText(label, n.x0 + 7, n.y0 + (twoLines ? 15 : h / 2));
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
