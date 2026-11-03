import { Button, Dialog, Heading, Modal, ModalOverlay } from "react-aria-components";
import { items, size } from "../format";
import { type Amount, type Chosen, tally } from "./selection";

interface Props {
  chosen: Chosen;
  isOpen: boolean;
  onOpenChange: (isOpen: boolean) => void;
  onConfirm: () => void;
}

/** Asks before a cleanup, saying what's deleted and what's quarantined. The answer starts at no. */
export function CleanDialog({ chosen, isOpen, onOpenChange, onConfirm }: Props) {
  const { deleted, quarantined } = tally(chosen);
  const amount = (group: Amount) => `${items(group.items)} (${size(group.size)})`;
  let question: string;
  let action: string;
  if (quarantined.items === 0) {
    question = `Delete ${amount(deleted)}?`;
    action = "Delete";
  } else if (deleted.items === 0) {
    question = `Quarantine ${amount(quarantined)}?`;
    action = "Quarantine";
  } else {
    question = `Delete ${amount(deleted)} and quarantine ${amount(quarantined)}?`;
    action = "Delete and quarantine";
  }

  return (
    <ModalOverlay isOpen={isOpen} onOpenChange={onOpenChange} isDismissable className="overlay">
      <Modal className="modal">
        <Dialog role="alertdialog" className="dialog">
          {({ close }) => (
            <>
              <Heading slot="title">{question}</Heading>
              <p>
                Each item is checked again first, and anything that's changed since the scan is
                skipped.
              </p>
              <p className="muted">
                Deleted items can't be restored, but they can be rebuilt, such as with npm install.
                Quarantined items can be restored until they're purged.
              </p>
              <div className="dialog-buttons">
                <Button className="button" onPress={close} autoFocus>
                  Cancel
                </Button>
                <Button className="button danger" onPress={onConfirm}>
                  {action}
                </Button>
              </div>
            </>
          )}
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}
