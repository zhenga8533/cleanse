import {
  Cell,
  Column,
  type SortDescriptor,
  Table,
  TableBody,
  TableHeader,
  Row as TableRow,
} from "react-aria-components";
import { bytes } from "../format";
import type { Row } from "./rows";

interface Props {
  label: string;
  rows: Row[];
  sort: SortDescriptor;
  onSortChange: (sort: SortDescriptor) => void;
  onOpen: (node: number) => void;
}

/** A folder's entries, sortable, opening a folder when its row is chosen. */
export function FolderTable({ label, rows, sort, onSortChange, onOpen }: Props) {
  const sorted = [...rows].sort(compare(sort));
  return (
    <Table
      aria-label={label}
      className="table"
      sortDescriptor={sort}
      onSortChange={onSortChange}
      onRowAction={(key) => {
        const opens = rows.find((row) => row.key === key)?.opens;
        if (opens !== undefined && opens !== null) {
          onOpen(opens);
        }
      }}
    >
      <TableHeader>
        {COLUMNS.map((column) => (
          <Column
            key={column.id}
            id={column.id}
            isRowHeader={column.id === "name"}
            allowsSorting={column.id !== "what"}
            className={column.number ? "column number" : "column"}
          >
            {({ sortDirection }) => (
              <>
                {column.title}
                {sortDirection && (
                  <span className="sort" aria-hidden="true">
                    {sortDirection === "ascending" ? " ▲" : " ▼"}
                  </span>
                )}
              </>
            )}
          </Column>
        ))}
      </TableHeader>
      <TableBody items={sorted}>
        {(row) => (
          <TableRow id={row.key} className={row.opens === null ? "table-row" : "table-row opens"}>
            <Cell className="cell">
              <span className="path">{row.name}</span>
              {row.incomplete && " *"}
            </Cell>
            <Cell className="cell number">{bytes(row.onDisk)}</Cell>
            <Cell className="cell number">{row.removable > 0 ? bytes(row.removable) : "–"}</Cell>
            <Cell className="cell">{row.what}</Cell>
          </TableRow>
        )}
      </TableBody>
    </Table>
  );
}

const COLUMNS = [
  { id: "name", title: "Name", number: false },
  { id: "size", title: "Size", number: true },
  { id: "removable", title: "Removable", number: true },
  { id: "what", title: "What it is", number: false },
] as const;

function compare(sort: SortDescriptor): (a: Row, b: Row) => number {
  const order = sort.direction === "ascending" ? 1 : -1;
  switch (sort.column) {
    case "name":
      return (a, b) => order * a.name.localeCompare(b.name);
    case "removable":
      return (a, b) => order * (a.removable - b.removable);
    default:
      return (a, b) => order * (a.onDisk - b.onDisk);
  }
}
