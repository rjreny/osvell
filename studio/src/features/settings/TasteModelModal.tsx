import { useEffect, useId, useRef } from "react";
import type { TasteModelInfo } from "../../platform/types/film";
import { TasteModelList } from "../films/RecsView";

export function TasteModelModal({
  open,
  models,
  selected,
  disabled,
  onClose,
  onPick,
}: {
  open: boolean;
  models: TasteModelInfo[];
  selected: string;
  disabled?: boolean;
  onClose: () => void;
  onPick: (id: string) => void;
}) {
  const titleId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    if (!open) return;

    const dialog = dialogRef.current;
    const focusableSelector = "button:not(:disabled)";
    dialog
      ?.querySelector<HTMLButtonElement>(
        'button[aria-pressed="true"]:not(:disabled), button:not(:disabled)',
      )
      ?.focus();

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        onCloseRef.current();
        return;
      }
      if (event.key !== "Tab" || !dialog) return;

      const controls = Array.from(
        dialog.querySelectorAll<HTMLButtonElement>(focusableSelector),
      );
      if (!controls.length) {
        event.preventDefault();
        dialog.focus();
        return;
      }

      const first = controls[0];
      const last = controls[controls.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      } else if (!dialog.contains(document.activeElement)) {
        event.preventDefault();
        first.focus();
      }
    }

    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [open]);

  if (!open) return null;

  return (
    <div
      ref={dialogRef}
      className="taste-model-modal"
      role="dialog"
      aria-modal="true"
      aria-labelledby={titleId}
      tabIndex={-1}
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div className="taste-model-modal-card solid-card">
        <header className="taste-model-modal-head">
          <div>
            <h2 id={titleId}>Choose recommendation model</h2>
            <p>Compare context and cost only when you need to change readers.</p>
          </div>
          <button
            type="button"
            className="text-btn"
            aria-label="Close recommendation model picker"
            onClick={onClose}
          >
            Close
          </button>
        </header>
        <TasteModelList
          models={models}
          selected={selected}
          disabled={disabled}
          onPick={(id) => {
            onPick(id);
            onClose();
          }}
        />
      </div>
    </div>
  );
}
