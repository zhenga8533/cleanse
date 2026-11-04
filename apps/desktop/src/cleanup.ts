import { createContext, useContext } from "react";
import { type Cleanup, USER_CLEANUP } from "./api";

/** The cleanup the screens inside work on: the user's own, unless the System section's. */
export const CleanupContext = createContext<Cleanup>(USER_CLEANUP);

export function useCleanup(): Cleanup {
  return useContext(CleanupContext);
}
