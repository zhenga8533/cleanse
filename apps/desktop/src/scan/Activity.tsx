import { Button, ProgressBar } from "react-aria-components";

interface Props {
  label: string;
  /** More about what's happening, such as the item being removed, shown under the bar. */
  detail?: string | null;
  /** What's been done, out of 1, or nothing if there's no telling how much is left. */
  done: number | null;
  onCancel: () => void;
  isCancelling: boolean;
}

/** A scan or cleanup that's running, and a way to stop it. */
export function Activity({ label, detail, done, onCancel, isCancelling }: Props) {
  return (
    <div className="activity">
      <ProgressBar
        className="progress"
        aria-label={label}
        valueLabel={detail ? `${label}: ${detail}` : label}
        minValue={0}
        maxValue={1}
        value={done ?? 0}
        isIndeterminate={done === null}
      >
        {({ percentage }) => (
          <>
            <span>{label}</span>
            <div className="bar">
              <div
                className={done === null ? "fill indeterminate" : "fill"}
                style={done === null ? undefined : { width: `${percentage ?? 0}%` }}
              />
            </div>
            {detail && (
              <span className="detail" title={detail}>
                {detail}
              </span>
            )}
          </>
        )}
      </ProgressBar>
      {/* What started it is turned off while it runs, so focus comes here instead. */}
      <Button className="button" onPress={onCancel} isDisabled={isCancelling} autoFocus>
        {isCancelling ? "Stopping…" : "Cancel"}
      </Button>
    </div>
  );
}
