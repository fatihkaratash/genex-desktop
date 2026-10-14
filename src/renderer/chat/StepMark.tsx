/** The round mark beside a line of the chat's step lists: ticked once its step is done, a dashed ring while it works, an empty ring before. */
import type { JSX } from "react";
import { Icon } from "../ui/icons.tsx";

/** The ring's line: dashed while under way, else solid — faint, or `clear` on a card's fill. */
function ringClass(working: boolean, clear: boolean): string {
  if (working) return "border-dashed border-ink-3";
  return clear ? "border-line-strong" : "border-line";
}

/**
 * A step's mark: a tick on the accent's tint when done, a dashed ring while it is under way, else
 * an empty ring — `clear` where it sits on the card's own fill, which the faint line vanishes into.
 * Decorative; the words say it.
 */
export function StepMark({
  done,
  working = false,
  clear = false,
}: {
  done: boolean;
  working?: boolean;
  clear?: boolean;
}): JSX.Element {
  if (done)
    return (
      <span
        aria-hidden="true"
        className="mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-full bg-accent-primary/12 text-accent-ink"
      >
        <Icon name="check" size={11} />
      </span>
    );
  return (
    <span
      aria-hidden="true"
      data-step-mark={working ? "working" : undefined}
      className={`mt-0.5 flex size-4 shrink-0 rounded-full border ${ringClass(working, clear)}`}
    />
  );
}
