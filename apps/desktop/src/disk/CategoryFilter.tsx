import {
  Button,
  Checkbox,
  CheckboxGroup,
  Dialog,
  DialogTrigger,
  Popover,
} from "react-aria-components";
import type { CategoryTotal } from "../bindings/CategoryTotal";
import { bytes } from "../format";

interface Props {
  totals: CategoryTotal[];
  /** The categories that count as removable, or none for all of them. */
  counted: string[];
  onChange: (counted: string[]) => void;
}

/** Which categories count as removable, as `--category` chooses for the CLI. */
export function CategoryFilter({ totals, counted, onChange }: Props) {
  let label: string;
  if (counted.length === 0) {
    label = "all categories";
  } else if (counted.length === 1) {
    label = totals.find((total) => total.category === counted[0])?.name ?? "1 category";
  } else {
    label = `${counted.length} categories`;
  }

  return (
    <DialogTrigger>
      <Button className="button" isDisabled={totals.length === 0}>
        Removable counts {label}
      </Button>
      <Popover className="popover" placement="bottom end">
        <Dialog className="dialog" aria-label="Categories that count as removable">
          <CheckboxGroup
            aria-label="Categories that count as removable"
            className="categories"
            value={counted}
            onChange={onChange}
          >
            {totals.map((total) => (
              <Checkbox key={total.category} value={total.category} className="checkbox">
                <span className="box" aria-hidden="true" />
                <span>{total.name}</span>
                <span className="muted">{bytes(total.size.onDisk)}</span>
              </Checkbox>
            ))}
          </CheckboxGroup>
          <p className="muted">With none checked, every category counts.</p>
          <Button
            className="button quiet"
            isDisabled={counted.length === 0}
            onPress={() => onChange([])}
          >
            Count every category
          </Button>
        </Dialog>
      </Popover>
    </DialogTrigger>
  );
}
