import { Button } from "react-aria-components";
import { pickFolders, removeFolder } from "../api";
import type { Folders } from "../bindings/Folders";

interface Props {
  folders: Folders;
  isDisabled: boolean;
  onChange: (folders: Folders) => void;
  onError: (error: unknown) => void;
}

/** The folders a scan covers, which only the system's folder picker can add to. */
export function FolderList({ folders, isDisabled, onChange, onError }: Props) {
  return (
    <section className="card" aria-labelledby="folders-heading">
      <h2 id="folders-heading">Folders to scan</h2>
      {folders.folders.length === 0 ? (
        <p className="muted">This system has no home folder to scan, so add a folder.</p>
      ) : (
        <ul className="folders">
          {folders.folders.map((folder, index) => (
            <li key={folder}>
              <span className="path">{folder}</span>
              {folders.home ? (
                <span className="muted">Your home folder</span>
              ) : (
                <Button
                  className="button quiet"
                  aria-label={`Remove ${folder}`}
                  isDisabled={isDisabled}
                  onPress={() => removeFolder(index).then(onChange, onError)}
                >
                  Remove
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
      <div className="row">
        <Button
          className="button"
          isDisabled={isDisabled}
          onPress={() => pickFolders().then(onChange, onError)}
        >
          Add folders…
        </Button>
        {folders.home && (
          <span className="muted">Folders you add are scanned instead of your home folder.</span>
        )}
      </div>
    </section>
  );
}
