import type { Risk } from "../bindings/Risk";
import type { Section } from "../bindings/Section";

/** Each section's heading and what it means, in the CLI's words. */
export const SECTIONS: Record<Section, { title: string; about: string; verdict: string }> = {
  removable: {
    title: "Can be removed",
    about:
      "Their apps recreate them, or a command rebuilds them. Items with warnings aren't selected to begin with.",
    verdict: "can be removed",
  },
  review: {
    title: "Needs your review",
    about:
      "Only you can decide about these, so none are selected. They go to quarantine, where they can be restored.",
    verdict: "needs your review",
  },
  advice: {
    title: "Best cleaned by their own tools",
    about: "Cleanse leaves these alone. Run the command shown for each instead.",
    verdict: "kept, since its own tool cleans it better",
  },
  kept: {
    title: "Kept",
    about: "A safety check or their rule keeps these, so they can't be removed.",
    verdict: "kept",
  },
};

export const RISKS: Record<Risk, string> = {
  safe: "Safe",
  regenerable: "Regenerable",
  review: "Review",
  protected: "Protected",
};

/** Whether a cleanup can choose items from `section`. */
export function removable(section: Section): boolean {
  return section === "removable" || section === "review";
}
