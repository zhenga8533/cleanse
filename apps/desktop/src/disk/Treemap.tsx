import { hierarchy, treemap, treemapSquarify } from "d3-hierarchy";
import { useEffect, useRef, useState } from "react";
import { bytes } from "../format";

export interface Tile {
  key: string;
  name: string;
  onDisk: number;
  removable: number;
  /** The folder clicking it opens, if any. */
  opens: number | null;
  /** It stands for the entries the list leaves out. */
  rest?: boolean;
}

const HEIGHT = 260;

interface Props {
  label: string;
  tiles: Tile[];
  onOpen: (node: number) => void;
}

/**
 * A folder's entries as rectangles sized by the space they take, with the removable part of each
 * shaded. It's a picture of the table beside it, which is how a keyboard or screen reader gets at
 * the same entries.
 */
export function Treemap({ label, tiles, onOpen }: Props) {
  const box = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(800);

  useEffect(() => {
    const element = box.current;
    if (!element || typeof ResizeObserver === "undefined") {
      return;
    }
    const observer = new ResizeObserver(([entry]) => {
      if (entry) {
        setWidth(Math.max(1, Math.floor(entry.contentRect.width)));
      }
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const root = hierarchy<{ tile: Tile | null; children?: { tile: Tile }[] }>({
    tile: null,
    children: tiles.filter((tile) => tile.onDisk > 0).map((tile) => ({ tile })),
  })
    .sum((node) => node.tile?.onDisk ?? 0)
    .sort((a, b) => (b.value ?? 0) - (a.value ?? 0));
  const leaves = treemap<{ tile: Tile | null }>()
    .size([width, HEIGHT])
    .tile(treemapSquarify)
    .paddingInner(2)
    .round(true)(root)
    .leaves();

  return (
    <div ref={box} className="treemap">
      <svg width={width} height={HEIGHT} role="img" aria-label={label}>
        {leaves.map((leaf) => {
          const tile = leaf.data.tile;
          if (!tile) {
            return null;
          }
          const w = leaf.x1 - leaf.x0;
          const h = leaf.y1 - leaf.y0;
          const share = Math.min(1, tile.removable / tile.onDisk);
          const opens = tile.opens;
          return (
            // biome-ignore lint/a11y/noStaticElementInteractions: the table offers the same to a keyboard.
            <g
              key={tile.key}
              transform={`translate(${leaf.x0},${leaf.y0})`}
              className={opens === null ? "tile" : "tile opens"}
              onClick={opens === null ? undefined : () => onOpen(opens)}
            >
              <title>
                {tile.removable > 0
                  ? `${tile.name}: ${bytes(tile.onDisk)}, ${bytes(tile.removable)} removable`
                  : `${tile.name}: ${bytes(tile.onDisk)}`}
              </title>
              <rect width={w} height={h} className={tile.rest ? "tile-box rest" : "tile-box"} />
              {share > 0 && (
                <rect y={h - h * share} width={w} height={h * share} className="tile-removable" />
              )}
              {w > 48 && h > 22 && (
                <text x={6} y={17} className="tile-name">
                  {truncate(tile.name, w)}
                </text>
              )}
              {w > 48 && h > 40 && (
                <text x={6} y={34} className="tile-size">
                  {bytes(tile.onDisk)}
                </text>
              )}
            </g>
          );
        })}
      </svg>
    </div>
  );
}

/** As much of `name` as fits in `width` pixels, roughly. */
function truncate(name: string, width: number): string {
  const room = Math.floor((width - 12) / 7);
  return name.length <= room ? name : `${name.slice(0, Math.max(1, room - 1))}…`;
}
