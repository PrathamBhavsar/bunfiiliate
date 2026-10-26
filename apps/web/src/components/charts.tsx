"use client";

import { useEffect, useRef, useState, type PointerEvent } from "react";

export type ChartPoint = { day: string; value: number };

const H = 160;
const PAD = { l: 48, r: 10, t: 8, b: 22 };
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** `2026-03-07` -> `Mar 7`. */
export const formatDay = (day: string) => `${MONTHS[Number(day.slice(5, 7)) - 1]} ${Number(day.slice(8, 10))}`;

/** Round a maximum up to 1, 2, 5 or 10 times a power of ten, so the axis ends on a clean number. */
function niceMax(max: number) {
  if (max <= 0) return 1;
  const pow = 10 ** Math.floor(Math.log10(max));
  const f = max / pow;
  return (f <= 1 ? 1 : f <= 2 ? 2 : f <= 5 ? 5 : 10) * pow;
}

function useWidth() {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(600);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => setWidth(Math.max(240, Math.floor(entries[0]?.contentRect.width ?? 600))));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, width] as const;
}

/**
 * One metric over days: a 2px line on a quiet grid, zero baseline, hover crosshair.
 * One measure per chart (never a second y-scale); `color` is a CSS color such as `var(--series-1)`.
 * Keyboard and screen-reader users read the same numbers from the table the page renders next to it.
 */
export function LineChart({
  title,
  color,
  points,
  format,
  formatAxis = format,
}: {
  title: string;
  color: string;
  points: ChartPoint[];
  format: (v: number) => string;
  formatAxis?: (v: number) => string;
}) {
  const [ref, width] = useWidth();
  const [hover, setHover] = useState<number | null>(null);
  const n = points.length;
  const total = points.reduce((a, p) => a + p.value, 0);
  const top = niceMax(Math.max(0, ...points.map((p) => p.value)));
  const plotW = width - PAD.l - PAD.r;
  const plotH = H - PAD.t - PAD.b;
  const x = (i: number) => PAD.l + (n === 1 ? plotW / 2 : (i * plotW) / (n - 1));
  const y = (v: number) => PAD.t + plotH - (v / top) * plotH;
  const ticks = [...new Set([0, 1, 2].map((i) => Math.round((top * i) / 2)))];
  const line = points.map((p, i) => `${i ? "L" : "M"}${x(i).toFixed(1)} ${y(p.value).toFixed(1)}`).join("");
  const shown = hover === null ? undefined : points[hover];
  const dayAt = (i: number) => formatDay(points[i]?.day ?? "");

  function move(e: PointerEvent<SVGSVGElement>) {
    const box = e.currentTarget.getBoundingClientRect();
    const i = n === 1 ? 0 : Math.round(((e.clientX - box.left - PAD.l) / plotW) * (n - 1));
    setHover(Math.min(n - 1, Math.max(0, i)));
  }

  return (
    <figure className="m-0 grid gap-1 rounded-lg border border-border bg-surface p-3">
      <figcaption className="flex items-baseline justify-between gap-2 text-xs">
        <span className="font-semibold">{title}</span>
        <span className="tabular-nums text-muted" aria-hidden>
          {shown ? `${formatDay(shown.day)}: ${format(shown.value)}` : `Total ${format(total)}`}
        </span>
      </figcaption>
      <div ref={ref}>
        <svg
          width={width}
          height={H}
          role="img"
          aria-label={`${title}, ${n} days from ${dayAt(0)} to ${dayAt(n - 1)}, total ${format(total)}`}
          className="touch-pan-y"
          onPointerMove={move}
          onPointerLeave={() => setHover(null)}
        >
          {ticks.map((t) => (
            <g key={t}>
              <line x1={PAD.l} x2={width - PAD.r} y1={y(t)} y2={y(t)} stroke="var(--border)" />
              <text x={PAD.l - 6} y={y(t) + 3.5} textAnchor="end" fontSize="10" fill="var(--muted)">
                {formatAxis(t)}
              </text>
            </g>
          ))}
          {[0, Math.floor((n - 1) / 2), n - 1]
            .filter((v, i, a) => a.indexOf(v) === i)
            .map((i) => (
              <text
                key={i}
                x={x(i)}
                y={H - 6}
                textAnchor={i === 0 && n > 1 ? "start" : i === n - 1 && n > 1 ? "end" : "middle"}
                fontSize="10"
                fill="var(--muted)"
              >
                {dayAt(i)}
              </text>
            ))}
          <path d={line} fill="none" stroke={color} strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" />
          {n === 1 && <circle cx={x(0)} cy={y(points[0]?.value ?? 0)} r={4} fill={color} />}
          {hover !== null && shown && (
            <>
              <line x1={x(hover)} x2={x(hover)} y1={PAD.t} y2={PAD.t + plotH} stroke="var(--muted)" strokeDasharray="3 3" />
              <circle cx={x(hover)} cy={y(shown.value)} r={4} fill={color} stroke="var(--surface)" strokeWidth={2} />
            </>
          )}
        </svg>
      </div>
    </figure>
  );
}
