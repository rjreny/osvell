import { useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";

const EDGE = 2;

function pageDelta(el: HTMLElement) {
  const rawGap = Number.parseFloat(getComputedStyle(el).columnGap);
  const gap = Number.isFinite(rawGap) ? rawGap : 14;
  const card = el.querySelector<HTMLElement>(":scope > *");
  const cardWidth = card?.getBoundingClientRect().width ?? 0;
  const stride = cardWidth > 0 ? cardWidth + gap : el.clientWidth;
  if (stride <= 0) return Math.max(el.clientWidth, 1);
  const count = Math.max(1, Math.floor((el.clientWidth + gap) / stride));
  return count * stride;
}

function Chevron({ direction }: { direction: "left" | "right" }) {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d={direction === "left" ? "M14.5 6 8.5 12l6 6" : "M9.5 6l6 6-6 6"} />
    </svg>
  );
}

export function ShelfTrack({ children }: { children: ReactNode }) {
  const trackRef = useRef<HTMLDivElement>(null);
  const [edges, setEdges] = useState({ overflow: false, left: false, right: false });

  useLayoutEffect(() => {
    const el = trackRef.current;
    if (!el) return undefined;
    const update = () => {
      const max = el.scrollWidth - el.clientWidth;
      const next = {
        overflow: max > EDGE,
        left: el.scrollLeft > EDGE,
        right: max - el.scrollLeft > EDGE,
      };
      setEdges((prev) =>
        prev.overflow === next.overflow && prev.left === next.left && prev.right === next.right ? prev : next,
      );
    };
    update();
    el.addEventListener("scroll", update, { passive: true });
    const resize = typeof ResizeObserver === "function" ? new ResizeObserver(update) : null;
    resize?.observe(el);
    const mutations = typeof MutationObserver === "function" ? new MutationObserver(update) : null;
    mutations?.observe(el, { childList: true });
    window.addEventListener("resize", update);
    return () => {
      el.removeEventListener("scroll", update);
      resize?.disconnect();
      mutations?.disconnect();
      window.removeEventListener("resize", update);
    };
  }, []);

  function scrollByDir(direction: -1 | 1) {
    const el = trackRef.current;
    if (!el) return;
    const left = direction * pageDelta(el);
    const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    if (typeof el.scrollBy === "function") el.scrollBy({ left, behavior: reduce ? "auto" : "smooth" });
    else el.scrollLeft += left;
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    if ((event.target as HTMLElement).closest(".shelf-arrow")) return;
    event.preventDefault();
    scrollByDir(event.key === "ArrowRight" ? 1 : -1);
  }

  return (
    <div className="shelf-row" onKeyDown={onKeyDown}>
      <div className="shelf-track" ref={trackRef}>
        {children}
      </div>
      {edges.overflow ? (
        <>
          <button
            type="button"
            className="shelf-arrow is-prev glass"
            aria-label="Previous films"
            disabled={!edges.left}
            onClick={() => scrollByDir(-1)}
          >
            <Chevron direction="left" />
          </button>
          <button
            type="button"
            className="shelf-arrow is-next glass"
            aria-label="Next films"
            disabled={!edges.right}
            onClick={() => scrollByDir(1)}
          >
            <Chevron direction="right" />
          </button>
        </>
      ) : null}
    </div>
  );
}

export function Shelf({
  title,
  action,
  children,
  empty,
  className = "",
}: {
  title: string;
  action?: ReactNode;
  children: ReactNode;
  empty?: ReactNode;
  className?: string;
}) {
  return (
    <section className={`shelf${className ? ` ${className}` : ""}`}>
      <header className="shelf-head">
        <h2>{title}</h2>
        {action}
      </header>
      {empty ? empty : <ShelfTrack>{children}</ShelfTrack>}
    </section>
  );
}
