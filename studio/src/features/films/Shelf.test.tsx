import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Shelf } from "./Shelf";

function mockTrackSize(scrollWidth: number, clientWidth: number) {
  vi.spyOn(HTMLElement.prototype, "scrollWidth", "get").mockReturnValue(scrollWidth);
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(clientWidth);
}

function renderShelf() {
  return render(
    <Shelf title="Recent from your log">
      <div className="film-card">One</div>
      <div className="film-card">Two</div>
    </Shelf>,
  );
}

describe("Shelf horizontal navigation", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("offers previous and next controls when the row is wider than the page", () => {
    mockTrackSize(900, 300);
    const { container } = renderShelf();
    const track = container.querySelector(".shelf-track") as HTMLDivElement;
    const scrollBy = vi.fn();
    track.scrollBy = scrollBy;

    expect(screen.getByRole("button", { name: "Previous films" })).toBeDisabled();
    const next = screen.getByRole("button", { name: "Next films" });
    expect(next).toBeEnabled();

    fireEvent.click(next);
    expect(scrollBy).toHaveBeenCalledWith({ left: 300, behavior: "smooth" });

    fireEvent.keyDown(screen.getByText("One"), { key: "ArrowLeft" });
    expect(scrollBy).toHaveBeenCalledWith({ left: -300, behavior: "smooth" });

    fireEvent.keyDown(next, { key: "ArrowRight" });
    expect(scrollBy).toHaveBeenCalledTimes(2);
  });

  it("hides the controls when every film fits", () => {
    mockTrackSize(300, 300);
    renderShelf();
    expect(screen.queryByRole("button", { name: "Next films" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Previous films" })).not.toBeInTheDocument();
  });
});
