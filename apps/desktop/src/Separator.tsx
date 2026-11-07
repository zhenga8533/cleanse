/**
 * The ": " between a path and what's said about it, where each has a line of its own. It isn't
 * shown, but text copied from the app keeps the two apart, and screen readers pause between them.
 */
export function Separator() {
  return <span className="visually-hidden">: </span>;
}
