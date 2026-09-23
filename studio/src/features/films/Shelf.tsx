import type { ReactNode } from "react";

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
      {empty ? empty : <div className="shelf-track">{children}</div>}
    </section>
  );
}
